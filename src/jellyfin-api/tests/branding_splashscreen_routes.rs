#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NamedConfigurationRepository, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Splashscreen Tests\", DeviceId=\"splashscreen-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_splashscreen_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn splashscreen_routes_match_official_persistence_and_authorization_contract() {
    let fixture = Fixture::new().await;
    let fallback_path = fixture.program_data.join("splashscreen.png");
    let fallback = png([30, 200, 30, 255]);
    std::fs::write(&fallback_path, &fallback).unwrap();

    for credential in [Credential::None, Credential::Device(&fixture.user_token)] {
        assert_eq!(
            fixture
                .request(
                    Method::GET,
                    "/Branding/Splashscreen",
                    credential,
                    None,
                    None
                )
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Branding/Splashscreen",
                Credential::Device("invalid-token"),
                None,
                None,
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for credential in [
        Credential::Device(&fixture.admin_token),
        Credential::ApiKey(&fixture.api_key),
    ] {
        let response = fixture
            .request(
                Method::GET,
                "/Branding/Splashscreen",
                credential,
                None,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(body(response).await, fallback);
    }

    fixture
        .configurations
        .save(
            "branding",
            json!({
                "SplashscreenEnabled": true,
                "LoginDisclaimer": "keep me",
                "SplashscreenLocation": null
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        body(
            fixture
                .request(
                    Method::GET,
                    "/Branding/Splashscreen",
                    Credential::None,
                    None,
                    None,
                )
                .await
        )
        .await,
        fallback
    );

    let uploaded = png([20, 30, 220, 255]);
    let encoded = format!(" \n{}\n", BASE64_STANDARD.encode(&uploaded));
    assert_eq!(
        fixture
            .request(
                Method::POST,
                "/Branding/Splashscreen",
                Credential::None,
                Some("image/png"),
                Some(encoded.clone()),
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                "/Branding/Splashscreen",
                Credential::Device(&fixture.user_token),
                Some("image/png"),
                Some(encoded.clone()),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                "/Branding/Splashscreen",
                Credential::Device(&fixture.admin_token),
                Some("text/plain"),
                Some(encoded.clone()),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                "/Branding/Splashscreen",
                Credential::Device(&fixture.admin_token),
                Some("image/png; charset=binary"),
                Some(encoded),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let upload_path = fixture.program_data.join("splashscreen-upload.png");
    assert_eq!(std::fs::read(&upload_path).unwrap(), uploaded);
    let branding = fixture.configurations.load("branding").await.unwrap();
    assert_eq!(branding.configuration["SplashscreenEnabled"], true);
    assert_eq!(branding.configuration["LoginDisclaimer"], "keep me");
    assert_eq!(
        branding.configuration["SplashscreenLocation"],
        upload_path.to_string_lossy().as_ref()
    );

    let response = fixture
        .request(
            Method::GET,
            "/Branding/Splashscreen?tag=splash-tag",
            Credential::None,
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], "\"splash-tag\"");
    assert_eq!(body(response).await, uploaded);
    let not_modified = fixture
        .request_with_if_none_match("/Branding/Splashscreen?tag=splash-tag", "\"splash-tag\"")
        .await;
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert!(body(not_modified).await.is_empty());

    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                "/Branding/Splashscreen",
                Credential::Device(&fixture.user_token),
                None,
                None,
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                "/Branding/Splashscreen",
                Credential::ApiKey(&fixture.api_key),
                None,
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(!upload_path.exists());
    let branding = fixture.configurations.load("branding").await.unwrap();
    assert_eq!(branding.configuration["SplashscreenLocation"], Value::Null);
    assert_eq!(
        body(
            fixture
                .request(
                    Method::GET,
                    "/Branding/Splashscreen",
                    Credential::None,
                    None,
                    None,
                )
                .await
        )
        .await,
        fallback
    );

    fixture.cleanup().await;
}

fn png(color: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 2, image::Rgba(color)))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
    ApiKey(&'a str),
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: axum::Router,
    configurations: NamedConfigurationRepository,
    program_data: std::path::PathBuf,
    admin_token: String,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new() -> Self {
        let (database_name, database) = test_database().await;
        let suffix = Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("jellyfin-splashscreen-{suffix}"));
        let program_data = root.join("programdata");
        std::fs::create_dir_all(&program_data).unwrap();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("splash-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("splash-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("splash-key-{suffix}"))
            .await
            .unwrap()
            .access_token;
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Splashscreen Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                &program_data,
                root.join("web"),
                root.join("cache/images"),
                root.join("cache"),
                root.join("metadata"),
            ),
        );
        Self {
            database_name,
            database: database.clone(),
            app,
            configurations: NamedConfigurationRepository::new(database),
            program_data,
            admin_token,
            user_token,
            api_key,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
        content_type: Option<&str>,
        body: Option<String>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        request = match credential {
            Credential::None => request,
            Credential::Device(token) => request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            ),
            Credential::ApiKey(token) => request.header("X-Emby-Token", token),
        };
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.unwrap_or_default())).unwrap())
            .await
            .unwrap()
    }

    async fn request_with_if_none_match(&self, uri: &str, tag: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(header::IF_NONE_MATCH, tag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            program_data,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let root = program_data.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(root);
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Splashscreen Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .unwrap()
        .access_token
}

async fn body(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .unwrap()
        .to_vec()
}

async fn test_database() -> (String, DatabaseConnection) {
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");
    administrator.close().await.unwrap();

    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    (database_name, database)
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
