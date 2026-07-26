use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"System Info Tests\", DeviceId=\"system-info-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_system_info_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn system_info_matches_official_first_time_or_authenticated_contract() {
    let fixture = Fixture::new().await;

    let anonymous_before_setup = body_json(
        fixture
            .request(Method::GET, "/System/Info", Credential::None)
            .await,
    )
    .await;
    assert_system_info(&anonymous_before_setup, false);

    let public_before_setup = body_json(
        fixture
            .request(Method::GET, "/System/Info/Public", Credential::None)
            .await,
    )
    .await;
    assert_public_system_info(&public_before_setup, false);

    assert_eq!(
        body_text(
            fixture
                .request(Method::GET, "/System/Ping", Credential::None)
                .await,
        )
        .await,
        "System Info Test Server"
    );
    assert_eq!(
        body_text(
            fixture
                .request(Method::POST, "/System/Ping", Credential::None)
                .await,
        )
        .await,
        "System Info Test Server"
    );

    assert_eq!(
        fixture
            .request(Method::POST, "/Startup/Complete", Credential::None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/System/Info", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let public_after_setup = body_json(
        fixture
            .request(Method::GET, "/System/Info/Public", Credential::None)
            .await,
    )
    .await;
    assert_public_system_info(&public_after_setup, true);

    let regular_user_info = body_json(
        fixture
            .request(
                Method::GET,
                "/System/Info",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_system_info(&regular_user_info, true);

    let api_key_info = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/System/Info?api_key={}", fixture.api_key_token),
                Credential::None,
            )
            .await,
    )
    .await;
    assert_system_info(&api_key_info, true);

    assert!(regular_user_info.get("server_name").is_none());
    fixture.cleanup().await;
}

fn assert_public_system_info(info: &Value, startup_completed: bool) {
    assert_eq!(info["ServerName"], "System Info Test Server");
    assert_eq!(info["LocalAddress"], "http://127.0.0.1:8096");
    assert_eq!(info["ProductName"], "Jellyfin Server");
    assert_eq!(info["StartupWizardCompleted"], startup_completed);
    assert_eq!(info["Id"].as_str().expect("server id").len(), 32);
    assert_eq!(info["OperatingSystem"], "");
    assert!(info.get("OperatingSystemDisplayName").is_none());
    assert!(info.get("ProgramDataPath").is_none());
    assert!(info.get("WebSocketPortNumber").is_none());
}

fn assert_system_info(info: &Value, startup_completed: bool) {
    assert_eq!(info["ServerName"], "System Info Test Server");
    assert_eq!(info["LocalAddress"], "http://127.0.0.1:8096");
    assert_eq!(info["ProductName"], "Jellyfin Server");
    assert_eq!(info["StartupWizardCompleted"], startup_completed);
    assert_eq!(info["Id"].as_str().expect("server id").len(), 32);
    assert_eq!(info["OperatingSystem"], "");
    assert_eq!(info["OperatingSystemDisplayName"], "");
    assert_eq!(info["PackageName"], Value::Null);
    assert_eq!(info["HasPendingRestart"], false);
    assert_eq!(info["IsShuttingDown"], false);
    assert_eq!(info["SupportsLibraryMonitor"], true);
    assert_eq!(info["WebSocketPortNumber"], 8096);
    assert_eq!(info["CompletedInstallations"], Value::Array(Vec::new()));
    assert_eq!(info["CanSelfRestart"], true);
    assert_eq!(info["CanLaunchWebBrowser"], false);
    assert_eq!(info["ProgramDataPath"], "programdata");
    assert_eq!(info["WebPath"], "web");
    assert_eq!(info["ItemsByNamePath"], "metadata");
    assert_eq!(info["CachePath"], "cache");
    assert_eq!(info["LogPath"], "logs");
    assert_eq!(info["InternalMetadataPath"], "metadata");
    assert_eq!(
        info["TranscodingTempPath"]
            .as_str()
            .expect("transcoding temp path")
            .ends_with("jellyfin-rust/transcodes"),
        true
    );
    assert_eq!(info["CastReceiverApplications"], Value::Array(Vec::new()));
    assert_eq!(info["HasUpdateAvailable"], false);
    assert_eq!(info["EncoderLocation"], "System");
    assert_eq!(info["SystemArchitecture"], "X64");
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    String::from_utf8(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    user_token: String,
    api_key_token: String,
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
        let users = UserService::new(database.clone());
        let user = users
            .create(&format!("system-info-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "System Info Tests",
                "1.0",
                "Test",
                format!("system-info-tests-{suffix}"),
            ))
            .await
            .unwrap()
            .access_token;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("system-info-key-{suffix}"))
            .await
            .unwrap()
            .access_token;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "System Info Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            user_token,
            api_key_token,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Credential::Device(token) = credential {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
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

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
