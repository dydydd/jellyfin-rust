use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, user, virtual_folder},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"First Time Policy Tests\", DeviceId=\"first-time-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn first_time_policy_matches_anonymous_and_elevated_route_matrix() {
    let fixture = Fixture::new().await;
    assert_incomplete_setup_access(&fixture).await;
    create_anonymous_real_directory_library(&fixture).await;
    complete_startup(&fixture, Credential::None).await;
    assert_completed_setup_access(&fixture).await;
    complete_startup(&fixture, Credential::ApiKeyHeader(&fixture.api_key_token)).await;
    fixture.cleanup().await;
}

async fn assert_incomplete_setup_access(fixture: &Fixture) {
    for uri in [
        "/Startup/Configuration",
        "/Startup/User",
        "/Startup/FirstUser",
        "/Library/VirtualFolders",
    ] {
        assert_eq!(
            fixture
                .send(Method::GET, uri, Credential::None, None)
                .await
                .status(),
            StatusCode::OK,
            "incomplete setup must allow anonymous {uri}"
        );
    }
}

async fn create_anonymous_real_directory_library(fixture: &Fixture) {
    let folder_name = format!("First Time {}", fixture.suffix);
    let create_uri = format!(
        "/Library/VirtualFolders?name={}&paths={}&refreshLibrary=true",
        encoded(&folder_name),
        encoded(&fixture.media_path)
    );
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &create_uri,
                Credential::None,
                Some(json!({ "LibraryOptions": { "Enabled": false } })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let response = fixture
        .send(
            Method::GET,
            "/Library/VirtualFolders",
            Credential::None,
            None,
        )
        .await;
    let folders = body_json(response).await;
    let folder = folders
        .as_array()
        .unwrap()
        .iter()
        .find(|folder| folder["Name"] == folder_name)
        .expect("anonymous startup request must persist the virtual folder");
    assert_eq!(folder["Locations"], json!([fixture.canonical_media_path]));
    assert_eq!(folder["LibraryOptions"]["Enabled"], false);
}

async fn assert_completed_setup_access(fixture: &Fixture) {
    for uri in ["/Startup/Configuration", "/Library/VirtualFolders"] {
        assert_eq!(
            fixture
                .send(Method::GET, uri, Credential::None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "completed setup must authenticate {uri}"
        );
        assert_eq!(
            fixture
                .send(
                    Method::GET,
                    uri,
                    Credential::Device(&fixture.user_token),
                    None,
                )
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "completed setup must elevate {uri}"
        );
        assert_eq!(
            fixture
                .send(
                    Method::GET,
                    uri,
                    Credential::Device(&fixture.admin_token),
                    None,
                )
                .await
                .status(),
            StatusCode::OK,
            "administrator device must access {uri}"
        );
        assert_eq!(
            fixture
                .send(
                    Method::GET,
                    uri,
                    Credential::ApiKeyHeader(&fixture.api_key_token),
                    None,
                )
                .await
                .status(),
            StatusCode::OK,
            "API key header must access {uri}"
        );

        let modern_query = format!("{uri}?ApiKey={}", encoded(&fixture.api_key_token));
        assert_eq!(
            fixture
                .send(Method::GET, &modern_query, Credential::None, None)
                .await
                .status(),
            StatusCode::OK,
            "ApiKey query must access {uri}"
        );
        let legacy_query = format!("{uri}?api_key={}", encoded(&fixture.api_key_token));
        assert_eq!(
            fixture
                .send(Method::GET, &legacy_query, Credential::None, None)
                .await
                .status(),
            StatusCode::OK,
            "api_key query must access {uri}"
        );
    }
}

async fn complete_startup(fixture: &Fixture, credential: Credential<'_>) {
    assert_eq!(
        fixture
            .send(Method::POST, "/Startup/Complete", credential, None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn startup_user_business_rules_remain_separate_from_authorization() {
    let fixture = Fixture::new().await;
    let missing_user_app = jellyfin_api::router(
        AppState::new(
            fixture.database.clone(),
            "Missing Startup User".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_startup_user(Uuid::new_v4()),
    );
    let response = send_to(
        &missing_user_app,
        Method::POST,
        "/Startup/User",
        Credential::None,
        Some(json!({ "Name": "admin", "Password": "first password" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    assert_eq!(
        fixture
            .send(
                Method::POST,
                "/Startup/User",
                Credential::None,
                Some(json!({
                    "Name": format!("configured-{}", fixture.suffix),
                    "Password": "first password"
                })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(
                Method::POST,
                "/Startup/User",
                Credential::None,
                Some(json!({ "Name": "attacker", "Password": "replacement" })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    complete_startup(&fixture, Credential::None).await;
    assert_eq!(
        fixture
            .send(
                Method::POST,
                "/Startup/User",
                Credential::Device(&fixture.admin_token),
                Some(json!({ "Name": "attacker", "Password": "replacement" })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "administrator authorization must not bypass the configured-password guard"
    );

    fixture.cleanup().await;
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
    ApiKeyHeader(&'a str),
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    suffix: String,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
    temp_root: std::path::PathBuf,
    media_path: String,
    canonical_media_path: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("first-time-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("first-time-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("first-time-key-{suffix}"))
            .await
            .expect("API key creation");
        let temp_root = std::env::temp_dir().join(format!("jellyfin-first-time-{suffix}"));
        let media = temp_root.join("media");
        std::fs::create_dir_all(&media).expect("real media directory");
        let media_path = media.to_string_lossy().into_owned();
        let canonical_media_path = std::fs::canonicalize(&media)
            .expect("canonical media directory")
            .to_string_lossy()
            .into_owned();
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "First Time Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_startup_user(admin.id),
        );
        Self {
            database,
            app,
            suffix,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            temp_root,
            media_path,
            canonical_media_path,
        }
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
        body: Option<Value>,
    ) -> axum::response::Response {
        send_to(&self.app, method, uri, credential, body).await
    }

    async fn cleanup(self) {
        virtual_folder::Entity::delete_many()
            .filter(virtual_folder::Column::Name.contains(&self.suffix))
            .exec(&self.database)
            .await
            .expect("virtual-folder cleanup");
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API-key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        std::fs::remove_dir_all(&self.temp_root).expect("directory cleanup");
    }
}

async fn send_to(
    app: &axum::Router,
    method: Method,
    uri: &str,
    credential: Credential<'_>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    request = match credential {
        Credential::None => request,
        Credential::Device(token) => request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        ),
        Credential::ApiKeyHeader(token) => request.header("x-emby-token", token),
    };
    let body = if let Some(value) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&value).unwrap())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "First Time Policy Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}
