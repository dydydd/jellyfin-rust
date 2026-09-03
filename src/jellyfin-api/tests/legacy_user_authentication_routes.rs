#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ActivityLogQuery, ActivityLogRepository, DatabaseConfig, DeviceQuery, DeviceRepository,
};
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Legacy Authentication Tests\", DeviceId=\"legacy-auth-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_legacy_authentication_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn legacy_user_authentication_route_delegates_to_password_session_flow() {
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
        exercise_legacy_user_authentication_route(&task_database_name).await;
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

async fn exercise_legacy_user_authentication_route(database_name: &str) {
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

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let user = users
        .create(&format!("legacy-auth-user-{suffix}"))
        .await
        .expect("user creation");
    users
        .set_password_hash(
            user.id,
            DefaultAuthenticationProvider::new().password_hash("correct-password"),
        )
        .await
        .expect("password hash persistence");
    let devices = DeviceRepository::new(database.clone());
    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Legacy Authentication Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    assert_eq!(
        post(&app, &format!("/Users/{}/Authenticate", user.id))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &app,
            &format!("/Users/{}/Authenticate", Uuid::new_v4()),
            &json!({ "Pw": "correct-password" }),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post_json(
            &app,
            &format!("/Users/{}/Authenticate", user.id),
            &json!({ "Pw": "wrong" }),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let failed = users.get(user.id).await.expect("failed login user reload");
    assert_eq!(failed.policy["InvalidLoginAttemptCount"], 1);
    assert_eq!(
        devices
            .query(&DeviceQuery {
                user_id: Some(user.id),
                ..DeviceQuery::default()
            })
            .await
            .expect("device query after failures")
            .total_record_count,
        0
    );

    let response = post_json(
        &app,
        &format!("/Users/{}/Authenticate?pw=wrong-password", user.id),
        &json!({ "Pw": "correct-password" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let authentication = body_json(response).await;
    assert_eq!(authentication["User"]["Id"], user.id.simple().to_string());
    assert_eq!(authentication["User"]["Name"], user.username);
    assert_eq!(
        authentication["SessionInfo"]["DeviceId"],
        "legacy-auth-tests"
    );
    assert_eq!(
        authentication["SessionInfo"]["Client"],
        "Legacy Authentication Tests"
    );
    assert_eq!(
        authentication["SessionInfo"]["Id"],
        jellyfin_session_id("Legacy Authentication Tests", "legacy-auth-tests")
    );
    assert_eq!(
        authentication["SessionInfo"]["UserId"],
        user.id.simple().to_string()
    );
    assert!(authentication["SessionInfo"]["Capabilities"].is_object());
    assert!(authentication["SessionInfo"]["PlayState"].is_object());
    let access_token = authentication["AccessToken"]
        .as_str()
        .expect("access token")
        .to_owned();
    assert!(!access_token.is_empty());
    let succeeded = users
        .get(user.id)
        .await
        .expect("successful login user reload");
    assert_eq!(succeeded.policy["InvalidLoginAttemptCount"], 0);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let logs = ActivityLogRepository::new(database.clone())
        .query(&ActivityLogQuery::default())
        .await
        .expect("activity log query");
    assert!(
        logs.items
            .iter()
            .any(|entry| entry.activity_type == "AuthenticationFailed")
    );
    assert!(
        logs.items
            .iter()
            .any(|entry| entry.activity_type == "AuthenticationSucceeded")
    );
    assert!(
        logs.items
            .iter()
            .any(|entry| entry.activity_type == "SessionStarted")
    );

    assert_eq!(
        devices
            .query(&DeviceQuery {
                user_id: Some(user.id),
                is_active: Some(true),
                ..DeviceQuery::default()
            })
            .await
            .expect("device query after success")
            .total_record_count,
        1
    );
    assert_eq!(
        get_me(&app, &access_token).await.status(),
        StatusCode::OK,
        "legacy access token should authenticate subsequent requests"
    );

    database.close().await.expect("database pool cleanup");
}

fn jellyfin_session_id(app_name: &str, device_id: &str) -> String {
    use md5::{Digest, Md5};
    use std::fmt::Write as _;

    let mut hasher = Md5::new();
    for unit in format!("{app_name}{device_id}").encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    let bytes = hasher.finalize();
    let mut result = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0], bytes[5], bytes[4], bytes[7], bytes[6]
    );
    for byte in &bytes[8..] {
        write!(result, "{byte:02x}").expect("writing a String cannot fail");
    }
    result
}

async fn post(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
                .header(header::AUTHORIZATION, AUTHORIZATION)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn post_json(app: &axum::Router, uri: &str, body: &Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
                .header(header::AUTHORIZATION, AUTHORIZATION)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn get_me(app: &axum::Router, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get("/Users/Me")
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn body_json(response: axum::response::Response) -> Value {
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
