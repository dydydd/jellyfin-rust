use std::path::PathBuf;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_user_api_key_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn api_keys_have_administrator_access_to_user_mutation_routes() {
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
        exercise_api_key_user_routes(&task_database_name).await;
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

async fn exercise_api_key_user_routes(database_name: &str) {
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
    users
        .create_initial_administrator(&format!("api-key-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let regular_user = users
        .create(&format!("api-key-regular-{suffix}"))
        .await
        .expect("regular user creation");
    let regular_token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            regular_user.id,
            "User API Key Tests",
            "1.0",
            "Test Device",
            format!("api-key-regular-device-{suffix}"),
        ))
        .await
        .expect("regular user session creation")
        .access_token;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("user-routes-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;

    let storage_root = temporary_storage_root(&suffix);
    tokio::fs::create_dir_all(&storage_root)
        .await
        .expect("temporary storage creation");
    let app = app(database.clone(), &storage_root);

    let created_name = format!("api-key-created-{suffix}");
    let response = json_request(
        &app,
        "POST",
        "/Users/New",
        &api_key,
        json!({ "Name": created_name }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("created user response body"),
    )
    .expect("created user response JSON");
    let target_id = Uuid::parse_str(created["Id"].as_str().expect("created user id"))
        .expect("created user UUID");

    let denied = json_request(
        &app,
        "POST",
        &format!("/Users?userId={target_id}"),
        &regular_token,
        json!({ "Name": "cross-user-denied", "Configuration": {} }),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let renamed = format!("api-key-renamed-{suffix}");
    let response = json_request(
        &app,
        "POST",
        &format!("/Users?userId={target_id}"),
        &api_key,
        json!({ "Name": renamed, "Configuration": {} }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        users.get(target_id).await.expect("renamed user").username,
        renamed
    );

    let response = json_request(
        &app,
        "POST",
        &format!("/Users/Configuration?userId={target_id}"),
        &api_key,
        json!({
            "AudioLanguagePreference": "eng",
            "DisplayMissingEpisodes": true
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let configured = users.get(target_id).await.expect("configured user");
    assert_eq!(configured.preferences["AudioLanguagePreference"], "eng");
    assert_eq!(configured.preferences["DisplayMissingEpisodes"], true);

    let response = json_request(
        &app,
        "POST",
        &format!("/Users/Password?userId={target_id}"),
        &api_key,
        json!({ "NewPw": "api-key-password" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        users
            .get(target_id)
            .await
            .expect("password-updated user")
            .password_hash
            .is_some_and(|hash| !hash.is_empty())
    );

    let response = image_request(
        &app,
        "POST",
        &format!("/UserImage?userId={target_id}"),
        &api_key,
        Body::from("cHJvZmlsZQ=="),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let image = users
        .profile_image(target_id)
        .await
        .expect("profile image lookup")
        .expect("persisted profile image");
    assert_eq!(
        tokio::fs::read(&image.path).await.expect("profile image"),
        b"profile"
    );

    let response = image_request(
        &app,
        "DELETE",
        &format!("/UserImage?userId={target_id}"),
        &api_key,
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        users
            .profile_image(target_id)
            .await
            .expect("cleared profile image lookup")
            .is_none()
    );

    let response = json_request(&app, "POST", "/Users/Configuration", &api_key, json!({})).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = empty_request(&app, "DELETE", &format!("/Users/{target_id}"), &api_key).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(users.get(target_id).await.is_err());

    database.close().await.expect("database pool cleanup");
    tokio::fs::remove_dir_all(&storage_root)
        .await
        .expect("temporary storage cleanup");
}

fn app(database: DatabaseConnection, storage_root: &std::path::Path) -> Router {
    jellyfin_api::router(
        AppState::new(
            database,
            "User API Key Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            storage_root.join("programdata"),
            storage_root.join("web"),
            storage_root.join("image-cache"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
        ),
    )
}

async fn json_request(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-emby-token", token)
                .body(Body::from(body.to_string()))
                .expect("JSON request"),
        )
        .await
        .expect("user route response")
}

async fn image_request(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Body,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "image/png")
                .header("x-emby-token", token)
                .body(body)
                .expect("image request"),
        )
        .await
        .expect("user image route response")
}

async fn empty_request(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("x-emby-token", token)
                .body(Body::empty())
                .expect("empty request"),
        )
        .await
        .expect("user route response")
}

fn temporary_storage_root(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("jellyfin-user-api-key-routes-{suffix}"))
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
