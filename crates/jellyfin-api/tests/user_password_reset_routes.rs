#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, entities::password_reset};
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Password Reset Tests\", DeviceId=\"password-reset-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_password_reset_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn forgot_password_routes_match_official_pin_flow_with_postgres_state() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_password_reset_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_password_reset_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Password Reset Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let suffix = Uuid::new_v4().simple().to_string();
    let username = format!("password-reset-user-{suffix}");
    let users = UserService::new(database.clone());
    let authentication = DefaultAuthenticationProvider::new();
    let user = users.create(&username).await.expect("user creation");
    users
        .set_password_hash(user.id, authentication.password_hash("old password"))
        .await
        .expect("initial password hash");

    assert_eq!(
        request(&app, "POST", "/Users/ForgotPassword", json!({}))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let unknown = body_json(
        request(
            &app,
            "POST",
            "/Users/ForgotPassword",
            json!({ "EnteredUsername": format!("missing-{suffix}") }),
        )
        .await,
    )
    .await;
    assert_eq!(unknown["Action"], "PinCode");
    assert!(
        unknown["PinFile"]
            .as_str()
            .unwrap()
            .starts_with("passwordreset")
    );
    assert_eq!(
        password_reset::Entity::find()
            .count(&database)
            .await
            .expect("reset count"),
        0
    );

    let started = body_json(
        request(
            &app,
            "POST",
            "/Users/ForgotPassword",
            json!({ "EnteredUsername": username.to_lowercase() }),
        )
        .await,
    )
    .await;
    assert_eq!(started["Action"], "PinCode");
    assert!(
        started["PinFile"]
            .as_str()
            .unwrap()
            .starts_with("passwordreset")
    );
    assert!(
        std::path::Path::new(started["PinFile"].as_str().unwrap())
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    );
    assert!(
        started["PinExpirationDate"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );

    let reset = password_reset::Entity::find()
        .filter(password_reset::Column::UserId.eq(user.id))
        .one(&database)
        .await
        .expect("reset lookup")
        .expect("reset row");
    assert_eq!(reset.user_name, username);
    assert_eq!(reset.pin.len(), 11);
    assert_eq!(reset.pin_compact, reset.pin.replace('-', ""));

    let wrong_pin = request(
        &app,
        "POST",
        "/Users/ForgotPassword/Pin",
        json!({ "Pin": "00-00-00-00" }),
    )
    .await;
    assert_eq!(wrong_pin.status(), StatusCode::NOT_FOUND);

    let redeemed = body_json(
        request(
            &app,
            "POST",
            "/Users/ForgotPassword/Pin",
            json!({ "Pin": reset.pin_compact }),
        )
        .await,
    )
    .await;
    assert_eq!(
        redeemed,
        json!({
            "Success": true,
            "UsersReset": [username]
        })
    );
    assert_eq!(
        password_reset::Entity::find()
            .count(&database)
            .await
            .expect("reset count after redeem"),
        0
    );

    assert_eq!(
        request(
            &app,
            "POST",
            "/Users/AuthenticateByName",
            json!({ "Username": &username, "Pw": "old password" }),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/Users/AuthenticateByName",
            json!({ "Username": &username, "Pw": reset.pin_compact }),
        )
        .await
        .status(),
        StatusCode::OK
    );

    database.close().await.expect("database pool cleanup");
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, AUTHORIZATION)
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
