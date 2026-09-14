#![allow(clippy::too_many_lines)]

use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice, entities::user_profile_image};
use sea_orm::{ConnectionTrait, EntityTrait};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User Image Index Tests\", DeviceId=\"user-image-index-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_user_image_index_";

#[tokio::test]
async fn user_image_mutation_index_uses_signed_int32_across_protocol_trees() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task was cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let temporary = TempDirectory::new();
    let suffix = Uuid::new_v4().simple().to_string();
    let user = UserService::new(database.clone())
        .create_initial_administrator(&format!("image-index-admin-{suffix}"))
        .await
        .expect("administrator");
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            user.id,
            "User Image Index Tests",
            "1.0",
            "Test",
            format!("user-image-index-{suffix}"),
        ))
        .await
        .expect("administrator session")
        .access_token;
    let state = AppState::new(
        database.clone(),
        "User Image Index Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    )
    .with_storage_paths(
        temporary.path().join("programdata"),
        temporary.path().join("web"),
        temporary.path().join("cache/images"),
        temporary.path().join("cache"),
        temporary.path().join("metadata"),
    );
    let jellyfin = jellyfin_api::router(state.clone());
    let emby = jellyfin_emby_api::router(state);

    for (app, prefix, expected_success) in [
        (&jellyfin, "", StatusCode::NO_CONTENT),
        (&jellyfin, "/api", StatusCode::NO_CONTENT),
        (&emby, "/emby", StatusCode::OK),
    ] {
        for (method, path, body) in [
            (
                Method::POST,
                format!("{prefix}/Users/{}/Images/Primary/{}", user.id, i32::MIN),
                Some("aW1hZ2U="),
            ),
            (
                Method::POST,
                format!("{prefix}/users/{}/images/Primary/{}", user.id, i32::MAX),
                Some("aW1hZ2U="),
            ),
            (
                Method::DELETE,
                format!("{prefix}/Users/{}/Images/Primary/-1", user.id),
                None,
            ),
            (
                Method::DELETE,
                format!("{prefix}/users/{}/images/Primary/{}", user.id, i32::MIN),
                None,
            ),
            (
                Method::POST,
                format!(
                    "{prefix}/Users/{}/Images/Primary/{}/Delete",
                    user.id,
                    i32::MAX
                ),
                None,
            ),
            (
                Method::POST,
                format!("{prefix}/users/{}/images/Primary/-1/delete", user.id),
                None,
            ),
        ] {
            let response = request(app, method, &path, Some(&token), body).await;
            assert_eq!(response.status(), expected_success, "{path}");
            assert_empty(response, &path).await;
            let persisted = user_profile_image::Entity::find_by_id(user.id)
                .one(&database)
                .await
                .expect("profile image lookup");
            assert_eq!(
                persisted.is_some(),
                body.is_some(),
                "indexed mutation persistence at {path}",
            );
        }

        for (method, path, body) in [
            (
                Method::POST,
                format!("{prefix}/Users/{}/Images/Primary/2147483648", user.id),
                Some("aW1hZ2U="),
            ),
            (
                Method::POST,
                format!("{prefix}/users/{}/images/Primary/-2147483649", user.id),
                Some("aW1hZ2U="),
            ),
            (
                Method::DELETE,
                format!("{prefix}/Users/{}/Images/Primary/2147483648", user.id),
                None,
            ),
            (
                Method::DELETE,
                format!("{prefix}/users/{}/images/Primary/-2147483649", user.id),
                None,
            ),
            (
                Method::POST,
                format!(
                    "{prefix}/Users/{}/Images/Primary/2147483648/Delete",
                    user.id
                ),
                None,
            ),
            (
                Method::POST,
                format!(
                    "{prefix}/users/{}/images/Primary/-2147483649/delete",
                    user.id
                ),
                None,
            ),
        ] {
            let response = request(app, method, &path, Some(&token), body).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }

        let unauthorized_path = format!(
            "{prefix}/Users/{}/Images/not-a-real-type/-1",
            Uuid::new_v4()
        );
        let response = request(
            app,
            Method::POST,
            &unauthorized_path,
            None,
            Some("not-base64"),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "authorization must precede image-type, target, and body validation at {unauthorized_path}",
        );

        let missing_target_path = format!("{prefix}/users/{}/images/Primary/-1", Uuid::new_v4());
        let response = request(
            app,
            Method::POST,
            &missing_target_path,
            Some(&token),
            Some("not-base64"),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "target lookup must precede body validation at {missing_target_path}",
        );
    }

    drop(emby);
    drop(jellyfin);
    database.close().await.expect("database cleanup");
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "image/png");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn assert_empty(response: axum::response::Response, path: &str) {
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("response bytes")
            .is_empty(),
        "{path}",
    );
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-user-image-index-routes-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("temporary directory creation");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
