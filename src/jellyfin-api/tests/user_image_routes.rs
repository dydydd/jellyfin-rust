#![allow(clippy::too_many_lines)]
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DatabaseConfig, DeviceRepository, NewDevice,
    entities::{user, user_profile_image},
};
use sea_orm::{ConnectionTrait, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User Image Tests\", DeviceId=\"user-image-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_user_image_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn user_image_routes_persist_base64_profile_images() {
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
        exercise_user_image_routes(&task_database_name).await;
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

async fn exercise_user_image_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let temporary = TempDirectory::new();
    let app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "User Image Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            temporary.path().join("programdata"),
            temporary.path().join("web"),
            temporary.path().join("cache/images"),
            temporary.path().join("cache"),
            temporary.path().join("metadata"),
        ),
    );

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("image-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("image-user-{suffix}"))
        .await
        .expect("user creation");
    let other = users
        .create(&format!("image-other-{suffix}"))
        .await
        .expect("other user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
    let other_token = session(&devices, other.id, &format!("other-{suffix}")).await;
    let png = png_fixture();
    let encoded_png = BASE64_STANDARD.encode(&png);

    let mut restricted_policy = valid_policy();
    restricted_policy.enable_user_preference_access = false;
    users
        .update_policy(user.id, &restricted_policy)
        .await
        .expect("disable preference access");
    assert_eq!(
        post_image(
            &app,
            "/UserImage",
            Some(&user_token),
            "image/png",
            &encoded_png,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    users
        .update_policy(user.id, &valid_policy())
        .await
        .expect("restore preference access");

    assert_eq!(
        post_image(&app, "/UserImage", None, "image/png", "Zmlyc3Q=")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_image(
            &app,
            "/UserImage",
            Some(&user_token),
            "text/plain",
            "Zmlyc3Q="
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_image(
            &app,
            &format!("/UserImage?userId={}", other.id),
            Some(&user_token),
            "image/png",
            "Zmlyc3Q="
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let response = post_image(
        &app,
        "/UserImage",
        Some(&user_token),
        "image/png; charset=utf-8",
        &encoded_png,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_empty(response).await;
    let first = user_profile_image::Entity::find_by_id(user.id)
        .one(&database)
        .await
        .expect("profile image lookup")
        .expect("profile image row");
    assert!(first.path.ends_with("profile.png"));
    assert_eq!(
        tokio::fs::read(&first.path)
            .await
            .expect("stored PNG bytes"),
        png
    );
    let first_tag = jellyfin_controller::image_cache_tag(&first.path, first.last_modified);
    let by_id = get_json(&app, &format!("/Users/{}", user.id), &admin_token).await;
    let current = get_json(&app, "/Users/Me", &user_token).await;
    assert_eq!(by_id["PrimaryImageTag"], first_tag);
    assert_eq!(current["PrimaryImageTag"], first_tag);

    let listed = get_json(&app, "/Users", &admin_token).await;
    let listed_user = listed
        .as_array()
        .expect("user list")
        .iter()
        .find(|dto| dto["Id"] == user.id.simple().to_string())
        .expect("profile image user in list");
    assert_eq!(listed_user["PrimaryImageTag"], first_tag);

    let authenticated = authenticate(&app, &user.username).await;
    assert_eq!(authenticated["User"]["PrimaryImageTag"], first_tag);
    assert_eq!(
        authenticated["SessionInfo"]["UserPrimaryImageTag"],
        first_tag
    );
    let sessions = get_json(&app, "/Sessions", &admin_token).await;
    let user_sessions = sessions
        .as_array()
        .expect("session list")
        .iter()
        .filter(|session| session["UserId"] == user.id.simple().to_string())
        .collect::<Vec<_>>();
    assert!(!user_sessions.is_empty());
    assert!(
        user_sessions
            .iter()
            .all(|session| session["UserPrimaryImageTag"] == first_tag)
    );

    assert_eq!(
        get_image(&app, axum::http::Method::GET, "/UserImage", None, &[])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get_image(
            &app,
            axum::http::Method::GET,
            &format!("/UserImage?userId={}", Uuid::nil()),
            None,
            &[],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get_image(
            &app,
            axum::http::Method::GET,
            "/UserImage",
            Some("invalid-token"),
            &[],
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let anonymous = get_image(
        &app,
        axum::http::Method::GET,
        &format!("/UserImage?userId={}", user.id),
        None,
        &[],
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::OK);
    assert_eq!(anonymous.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        to_bytes(anonymous.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
        png.as_slice()
    );

    let head = get_image(
        &app,
        axum::http::Method::HEAD,
        "/UserImage",
        Some(&user_token),
        &[],
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH],
        png.len().to_string().as_str()
    );
    assert!(
        to_bytes(head.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );

    let jpeg = get_image(
        &app,
        axum::http::Method::GET,
        &format!(
            "/Users/{}/Images/not-a-real-type/-999?format=jpg&width=1&quality=1",
            user.id
        ),
        None,
        &[],
    )
    .await;
    assert_eq!(jpeg.status(), StatusCode::OK);
    assert_eq!(jpeg.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert!(
        !to_bytes(jpeg.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );

    let tagged = get_image(
        &app,
        axum::http::Method::GET,
        &format!("/Users/{}/Images/Primary?tag=profile-tag", user.id),
        None,
        &[],
    )
    .await;
    assert_eq!(tagged.status(), StatusCode::OK);
    assert_eq!(tagged.headers()[header::ETAG], "\"profile-tag\"");
    let not_modified = get_image(
        &app,
        axum::http::Method::GET,
        &format!("/UserImage?userId={}&tag=profile-tag", user.id),
        None,
        &[(header::IF_NONE_MATCH.as_str(), "\"profile-tag\"")],
    )
    .await;
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert_empty(not_modified).await;

    assert_eq!(
        get_image(
            &app,
            axum::http::Method::GET,
            &format!("/UserImage?userId={}", other.id),
            None,
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let response = post_image(
        &app,
        &format!("/Users/{}/Images/Primary/0", user.id),
        Some(&user_token),
        "image/jpeg",
        "c2Vjb25k",
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_empty(response).await;
    assert!(
        tokio::fs::metadata(&first.path).await.is_err(),
        "replacing a profile image should remove the old file"
    );
    let second = user_profile_image::Entity::find_by_id(user.id)
        .one(&database)
        .await
        .expect("profile image lookup")
        .expect("profile image row");
    assert!(second.path.ends_with("profile.jpg"));
    assert_eq!(
        tokio::fs::read(&second.path)
            .await
            .expect("stored JPEG bytes"),
        b"second"
    );
    let second_tag = jellyfin_controller::image_cache_tag(&second.path, second.last_modified);
    assert_ne!(second_tag, first_tag);
    assert_eq!(
        get_json(&app, &format!("/Users/{}", user.id), &admin_token).await["PrimaryImageTag"],
        second_tag
    );

    assert_eq!(
        delete(&app, "/UserImage", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        delete(
            &app,
            &format!("/UserImage?userId={}", user.id),
            Some(&other_token)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        delete(
            &app,
            &format!("/UserImage?userId={}", Uuid::new_v4()),
            Some(&admin_token)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let response = delete(
        &app,
        &format!("/UserImage?userId={}", user.id),
        Some(&admin_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_empty(response).await;
    assert!(
        tokio::fs::metadata(&second.path).await.is_err(),
        "deleting a profile image should remove the persisted file"
    );
    assert!(
        user_profile_image::Entity::find_by_id(user.id)
            .one(&database)
            .await
            .expect("profile image lookup")
            .is_none()
    );
    assert!(
        get_json(&app, &format!("/Users/{}", user.id), &admin_token)
            .await
            .get("PrimaryImageTag")
            .is_none()
    );

    assert_eq!(
        delete(
            &app,
            &format!("/Users/{}/Images/Primary", user.id),
            Some(&admin_token)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    user::Entity::delete_many()
        .exec(&database)
        .await
        .expect("user image route user cleanup");
    database.close().await.expect("database pool cleanup");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "User Image Tests",
            "1.0",
            "Test",
            format!("user-image-tests-{suffix}"),
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn post_image(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    content_type: &'static str,
    body: &str,
) -> axum::response::Response {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, content_type);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn get_image(
    app: &Router,
    method: axum::http::Method,
    uri: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn delete(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::delete(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn get_json(app: &Router, uri: &str, token: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::get(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .expect("JSON request"),
        )
        .await
        .expect("JSON response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("JSON response body"),
    )
    .expect("JSON response value")
}

async fn authenticate(app: &Router, username: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::post("/Users/AuthenticateByName")
                .header(header::AUTHORIZATION, AUTHORIZATION)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "Username": username, "Pw": "" }).to_string(),
                ))
                .expect("authentication request"),
        )
        .await
        .expect("authentication response");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("authentication response body"),
    )
    .expect("authentication response value")
}

async fn assert_empty(response: axum::response::Response) {
    let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .expect("response body bytes");
    assert!(bytes.is_empty());
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-user-image-routes-{}",
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
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

fn png_fixture() -> Vec<u8> {
    let image = RgbaImage::from_pixel(3, 2, Rgba([20, 100, 200, 255]));
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("PNG fixture encoding");
    bytes.into_inner()
}

fn valid_policy() -> jellyfin_model::UserPolicy {
    jellyfin_model::UserPolicy {
        authentication_provider_id: Some(
            jellyfin_model::UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
        ),
        password_reset_provider_id: Some(
            jellyfin_model::UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
        ),
        ..jellyfin_model::UserPolicy::default()
    }
}
