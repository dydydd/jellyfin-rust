use std::fmt::Write as _;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DeviceRepository, NewDevice,
    entities::{device, user},
};
use md5::{Digest, Md5};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Capability Tests\", DeviceId=\"capability-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn session_capabilities_are_persisted_and_projected_from_postgres_jsonb() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request("POST", "/Sessions/Capabilities", None, Body::empty())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let query_response = fixture
        .request(
            "POST",
            "/Sessions/Capabilities?playableMediaTypes=Video,Audio&supportedCommands=Play,DisplayMessage&supportsMediaControl=true&supportsPersistentIdentifier=false",
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_eq!(query_response.status(), StatusCode::NO_CONTENT);
    assert_query_capabilities(&fixture.sessions().await);

    let full_response = fixture
        .request(
            "POST",
            &format!("/Sessions/Capabilities/Full?id={}", fixture.session_id),
            Some(&fixture.token),
            Body::from(
                json!({
                    "PlayableMediaTypes": ["Book"],
                    "SupportedCommands": ["GoHome", "SetVolume"],
                    "SupportsMediaControl": false,
                    "SupportsPersistentIdentifier": true,
                    "DeviceProfile": {
                        "Name": "Capabilities Profile",
                        "MaxStreamingBitrate": 123_456
                    },
                    "AppStoreUrl": "https://example.test/app",
                    "IconUrl": "https://example.test/icon.png"
                })
                .to_string(),
            ),
        )
        .await;
    assert_eq!(full_response.status(), StatusCode::NO_CONTENT);
    assert_full_capabilities(&fixture.sessions().await);

    let invalid_id_response = fixture
        .request(
            "POST",
            "/Sessions/Capabilities?id=not-this-session",
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_eq!(invalid_id_response.status(), StatusCode::BAD_REQUEST);

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    token: String,
    session_id: String,
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
        let user = users
            .create(&format!("capabilities-user-{suffix}"))
            .await
            .expect("user creation");
        let device_id = format!("capabilities-device-{suffix}");
        let devices = DeviceRepository::new(database.clone());
        let session = devices
            .create_session(NewDevice::new(
                user.id,
                "Jellyfin Web",
                "10.10.0",
                "Browser",
                &device_id,
            ))
            .await
            .expect("session creation");
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Session Capability Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            token: session.access_token,
            session_id: jellyfin_session_id("Jellyfin Web", &device_id),
        }
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Body,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
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

    async fn sessions(&self) -> Value {
        let response = self
            .request("GET", "/Sessions", Some(&self.token), Body::empty())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    }

    async fn cleanup(self) {
        device::Entity::delete_many()
            .filter(device::Column::AccessToken.eq(self.token))
            .exec(&self.database)
            .await
            .expect("test device cleanup");
        user::Entity::delete_by_id(self.user_id)
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

fn assert_query_capabilities(sessions: &Value) {
    let session = only_session(sessions);
    assert_eq!(session["PlayableMediaTypes"], json!(["Video", "Audio"]));
    assert_eq!(
        session["SupportedCommands"],
        json!(["Play", "DisplayMessage"])
    );
    assert_eq!(session["SupportsMediaControl"], false);
    assert_eq!(session["SupportsRemoteControl"], false);
    assert_eq!(
        session["Capabilities"]["PlayableMediaTypes"],
        json!(["Video", "Audio"])
    );
    assert_eq!(
        session["Capabilities"]["SupportedCommands"],
        json!(["Play", "DisplayMessage"])
    );
    assert_eq!(session["Capabilities"]["SupportsMediaControl"], true);
    assert_eq!(
        session["Capabilities"]["SupportsPersistentIdentifier"],
        false
    );
}

fn assert_full_capabilities(sessions: &Value) {
    let session = only_session(sessions);
    assert_eq!(session["PlayableMediaTypes"], json!(["Book"]));
    assert_eq!(session["SupportedCommands"], json!(["GoHome", "SetVolume"]));
    assert_eq!(session["SupportsRemoteControl"], false);
    assert_eq!(
        session["Capabilities"]["DeviceProfile"]["Name"],
        "Capabilities Profile"
    );
    assert_eq!(
        session["Capabilities"]["DeviceProfile"]["MaxStreamingBitrate"],
        123_456
    );
    assert_eq!(
        session["Capabilities"]["AppStoreUrl"],
        "https://example.test/app"
    );
    assert_eq!(
        session["Capabilities"]["IconUrl"],
        "https://example.test/icon.png"
    );
}

fn only_session(sessions: &Value) -> &Value {
    let sessions = sessions.as_array().expect("sessions must be an array");
    assert_eq!(sessions.len(), 1);
    &sessions[0]
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn jellyfin_session_id(app_name: &str, device_id: &str) -> String {
    let key = format!("{app_name}{device_id}");
    let mut hasher = Md5::new();
    for unit in key.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    let digest = hasher.finalize();
    let bytes = digest.as_slice();
    let mut result = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0], bytes[5], bytes[4], bytes[7], bytes[6]
    );
    for byte in &bytes[8..] {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}
