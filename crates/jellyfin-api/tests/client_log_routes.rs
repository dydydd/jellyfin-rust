use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"HeaderClient\", DeviceId=\"client-log-device\", Device=\"Test\", Version=\"9.0.0\"";
const DATABASE_PREFIX: &str = "jellyfin_client_log_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn client_log_document_route_matches_official_contract() {
    let fixture = Fixture::new().await;

    let response = fixture.post(None, "/Document", b"anonymous").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = fixture
        .post(Some(&fixture.user_token), "/Document", b"device payload")
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = body_json(response).await;
    let device_file = body["FileName"].as_str().unwrap();
    assert!(device_file.starts_with("upload_HeaderClient_9.0.0_"));
    assert!(body.get("file_name").is_none());
    assert_eq!(
        fs::read(fixture.log_directory.path().join(device_file)).unwrap(),
        b"device payload"
    );

    let api_key_uri = format!("/Document?api_key={}", fixture.api_key_token);
    let response = fixture.post(None, &api_key_uri, b"api key payload").await;
    assert_eq!(response.status(), StatusCode::OK);
    let api_key_file = body_json(response).await["FileName"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(api_key_file.starts_with(&format!("upload_{}_apikey_", fixture.api_key_name)));
    assert_eq!(
        fs::read(fixture.log_directory.path().join(api_key_file)).unwrap(),
        b"api key payload"
    );

    let too_large = fixture
        .post_with_content_length(Some(&fixture.user_token), "/Document", b"", 1_000_001)
        .await;
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);

    fixture
        .server_configuration
        .update_client_log_upload(false)
        .await
        .unwrap();
    let disabled = fixture
        .post(Some(&fixture.user_token), "/Document", b"blocked")
        .await;
    assert_eq!(disabled.status(), StatusCode::FORBIDDEN);
    assert_eq!(log_file_count(fixture.log_directory.path()), 2);

    fixture.cleanup().await;
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn log_file_count(path: &Path) -> usize {
    fs::read_dir(path)
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_file())
        .count()
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    log_directory: TempDirectory,
    server_configuration: ServerConfigurationRepository,
    user_token: String,
    api_key_token: String,
    api_key_name: String,
}

impl Fixture {
    async fn new() -> Self {
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

        let suffix = Uuid::new_v4().simple().to_string();
        let user = UserService::new(database.clone())
            .create(&format!("client-log-user-{suffix}"))
            .await
            .unwrap();
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "StoredClient",
                "1.0.0",
                "Test",
                "client-log-device",
            ))
            .await
            .unwrap()
            .access_token;
        let api_key_name = format!("client-log-key-{suffix}");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&api_key_name)
            .await
            .unwrap();
        let log_directory = TempDirectory::new();
        let server_configuration = ServerConfigurationRepository::new(database.clone());
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Client Log Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_log_directory(log_directory.path()),
        );

        Self {
            database_name,
            database,
            app,
            log_directory,
            server_configuration,
            user_token,
            api_key_token: api_key.access_token,
            api_key_name,
        }
    }

    async fn post(
        &self,
        token: Option<&str>,
        uri: &str,
        body: &'static [u8],
    ) -> axum::response::Response {
        self.post_with_headers(token, uri, Body::from(body)).await
    }

    async fn post_with_content_length(
        &self,
        token: Option<&str>,
        uri: &str,
        body: &'static [u8],
        content_length: usize,
    ) -> axum::response::Response {
        let mut request = Request::post(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(
                request
                    .header(header::CONTENT_LENGTH, content_length)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_with_headers(
        &self,
        token: Option<&str>,
        uri: &str,
        body: Body,
    ) -> axum::response::Response {
        let mut request = Request::post(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
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

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-client-log-route-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
