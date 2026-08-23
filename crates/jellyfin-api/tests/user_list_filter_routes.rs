#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice};
use jellyfin_model::UserPolicy;
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User List Filter Tests\", DeviceId=\"user-list-filter-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_user_list_filter_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn users_route_filters_hidden_and_disabled_users_in_postgres() {
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
        exercise_user_list_filter_routes(&task_database_name).await;
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

async fn exercise_user_list_filter_routes(database_name: &str) {
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
    let admin = users
        .create_initial_administrator(&format!("user-list-admin-{suffix}"))
        .await
        .expect("administrator creation");
    users
        .update_policy(admin.id, &policy(true, false, false))
        .await
        .expect("administrator visibility policy");

    let visible_enabled = users
        .create(&format!("user-list-visible-enabled-{suffix}"))
        .await
        .expect("visible enabled user");
    users
        .update_policy(visible_enabled.id, &policy(false, false, false))
        .await
        .expect("visible enabled policy");
    let hidden_enabled = users
        .create(&format!("user-list-hidden-enabled-{suffix}"))
        .await
        .expect("hidden enabled user");
    users
        .update_policy(hidden_enabled.id, &policy(false, true, false))
        .await
        .expect("hidden enabled policy");
    let visible_disabled = users
        .create(&format!("user-list-visible-disabled-{suffix}"))
        .await
        .expect("visible disabled user");
    users
        .update_policy(visible_disabled.id, &policy(false, false, true))
        .await
        .expect("visible disabled policy");
    let hidden_disabled = users
        .create(&format!("user-list-hidden-disabled-{suffix}"))
        .await
        .expect("hidden disabled user");
    users
        .update_policy(hidden_disabled.id, &policy(false, true, true))
        .await
        .expect("hidden disabled policy");

    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, visible_enabled.id, &format!("user-{suffix}")).await;
    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "User List Filter Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    assert_eq!(
        get(&app, "/Users", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, "/Users?isHidden=not-a-bool", Some(&admin_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let all = user_names(&get_json(&app, "/Users", &admin_token).await);
    assert_eq!(
        all,
        vec![
            format!("user-list-admin-{suffix}"),
            format!("user-list-hidden-disabled-{suffix}"),
            format!("user-list-hidden-enabled-{suffix}"),
            format!("user-list-visible-disabled-{suffix}"),
            format!("user-list-visible-enabled-{suffix}"),
        ]
    );

    assert_eq!(
        user_names(&get_json(&app, "/Users?isHidden=false", &admin_token).await),
        vec![
            format!("user-list-admin-{suffix}"),
            format!("user-list-visible-disabled-{suffix}"),
            format!("user-list-visible-enabled-{suffix}"),
        ]
    );
    assert_eq!(
        user_names(&get_json(&app, "/Users?IsHidden=true", &admin_token).await),
        vec![
            format!("user-list-hidden-disabled-{suffix}"),
            format!("user-list-hidden-enabled-{suffix}"),
        ]
    );
    assert_eq!(
        user_names(&get_json(&app, "/Users?isDisabled=false", &admin_token).await),
        vec![
            format!("user-list-admin-{suffix}"),
            format!("user-list-hidden-enabled-{suffix}"),
            format!("user-list-visible-enabled-{suffix}"),
        ]
    );
    assert_eq!(
        user_names(&get_json(&app, "/Users?isHidden=false&isDisabled=true", &admin_token,).await,),
        vec![format!("user-list-visible-disabled-{suffix}")]
    );
    assert_eq!(
        get(&app, "/Users?isHidden=false", Some(&user_token))
            .await
            .status(),
        StatusCode::OK
    );

    database.close().await.expect("database pool cleanup");
}

fn policy(is_administrator: bool, is_hidden: bool, is_disabled: bool) -> UserPolicy {
    UserPolicy {
        is_administrator,
        is_hidden,
        is_disabled,
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let response = get(app, uri, Some(token)).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn user_names(response: &Value) -> Vec<String> {
    response
        .as_array()
        .expect("user array")
        .iter()
        .map(|user| user["Name"].as_str().expect("user name").to_owned())
        .collect()
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "User List Filter Tests",
            "1.0",
            "Test",
            format!("user-list-filter-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
