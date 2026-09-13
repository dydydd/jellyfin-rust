use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::DatabaseConfig;
use jellyfin_model::UserPolicy;
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_emby_public_users_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn emby_public_users_are_anonymous_and_case_insensitive_without_changing_jellyfin() {
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
        exercise_public_user_routes(&task_database_name).await;
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

async fn exercise_public_user_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
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
        .create(&format!("emby-public-{suffix}"))
        .await
        .expect("public user creation");
    let policy = UserPolicy {
        is_hidden: false,
        is_disabled: false,
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    users
        .update_policy(user.id, &policy)
        .await
        .expect("public user policy");

    let emby = jellyfin_emby_api::router(AppState::new(
        database.clone(),
        "Emby Public User Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let mut emby_bodies = Vec::new();
    for route in [
        "/emby/Users/Public",
        "/emby/users/public",
        "/emby/uSeRs/pUbLiC",
    ] {
        emby_bodies.push(get_json(&emby, route).await);
    }
    assert!(emby_bodies.windows(2).all(|bodies| bodies[0] == bodies[1]));
    assert_sdk_decodable_user_array(&emby_bodies[0], user.id, &user.username);

    // This adapter is scoped to the nested Emby tree. The canonical and
    // lowercase Jellyfin public-login bootstrap routes keep their existing
    // handler and response unchanged.
    let jellyfin = jellyfin_api::router(AppState::new(
        database.clone(),
        "Jellyfin Public User Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let canonical = get_json(&jellyfin, "/Users/Public").await;
    let lowercase = get_json(&jellyfin, "/users/public").await;
    assert_eq!(canonical, lowercase);
    assert_sdk_decodable_user_array(&canonical, user.id, &user.username);

    database.close().await.expect("database pool cleanup");
}

fn assert_sdk_decodable_user_array(value: &Value, user_id: Uuid, username: &str) {
    let users = value
        .as_array()
        .expect("generated Android and Swift clients decode UserDto[]");
    let user = users
        .iter()
        .find(|user| user["Id"] == user_id.simple().to_string())
        .expect("public user must be present");
    assert_eq!(user["Name"], username);
    assert!(user["Id"].is_string());
    assert!(user["HasPassword"].is_boolean());
    assert!(user["HasConfiguredPassword"].is_boolean());
    assert!(user["Configuration"].is_object());
    assert!(user["Policy"].is_object());
}

async fn get_json(app: &axum::Router, uri: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .expect("public users response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("public users response body"),
    )
    .expect("public users JSON response")
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
