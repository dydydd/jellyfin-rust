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

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Capability Tests\", DeviceId=\"emby-capability-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn generated_emby_capabilities_contract_is_persisted_without_leaking_to_jellyfin() {
    let fixture = Fixture::new().await;

    let unauthorized = fixture
        .request(
            "POST",
            "/emby/Sessions/Capabilities?SupportsSync=not-a-bool",
            None,
            Body::empty(),
        )
        .await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    for path in [
        "/emby/Sessions/Capabilities",
        "/emby/Sessions/Capabilities/Full",
    ] {
        let response = fixture
            .request("POST", path, Some(&fixture.token), Body::empty())
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }

    let basic = fixture
        .request(
            "POST",
            &format!(
                "/emby/sEsSiOnS/cApAbIlItIeS?ID=wrong&id={}&PlayableMediaTypes=Video&pLaYaBlEmEdIaTyPeS=Audio,Book&SupportedCommands=Play&sUpPoRtEdCoMmAnDs=GoHome,DisplayMessage&SupportsMediaControl=invalid&supportsmEdIaCoNtRoL=TRUE&SupportsSync=invalid&sUpPoRtSsYnC=TrUe&ignored=value",
                fixture.session_id
            ),
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_empty_success(basic, StatusCode::OK).await;
    let stored = fixture.capabilities().await;
    assert_eq!(stored["PlayableMediaTypes"], json!(["Audio", "Book"]));
    assert_eq!(
        stored["SupportedCommands"],
        json!(["GoHome", "DisplayMessage"])
    );
    assert_eq!(stored["SupportsMediaControl"], true);
    assert_eq!(stored["SupportsSync"], true);
    assert!(stored.get("ignored").is_none());

    let full = fixture
        .request(
            "POST",
            &format!(
                "/emby/SeSsIoNs/CaPaBiLiTiEs/FuLl?ID=wrong&iD={}",
                fixture.session_id
            ),
            Some(&fixture.token),
            Body::from(
                r#"{
                    "PlayableMediaTypes":["Video"],
                    "playablemediatypes":["Audio","Photo"],
                    "SupportedCommands":["Play"],
                    "supportedcommands":["SetVolume"],
                    "SupportsMediaControl":false,
                    "supportsmediacontrol":true,
                    "PushToken":42,
                    "pushtoken":"push-final",
                    "PushTokenType":"fcm",
                    "SupportsSync":"invalid",
                    "supportssync":true,
                    "AppId":"mobile-app",
                    "IconUrl":"https://example.test/icon.png",
                    "UnknownProperty":{"nested":true}
                }"#,
            ),
        )
        .await;
    assert_empty_success(full, StatusCode::OK).await;
    let stored = fixture.capabilities().await;
    assert_eq!(stored["PlayableMediaTypes"], json!(["Audio", "Photo"]));
    assert_eq!(stored["SupportedCommands"], json!(["SetVolume"]));
    assert_eq!(stored["SupportsMediaControl"], true);
    assert_eq!(stored["PushToken"], "push-final");
    assert_eq!(stored["PushTokenType"], "fcm");
    assert_eq!(stored["SupportsSync"], true);
    assert_eq!(stored["AppId"], "mobile-app");
    assert!(stored.get("UnknownProperty").is_none());

    // The short generated operation does not carry push or app identifiers;
    // updating SupportsSync must retain those protocol-owned fields.
    let short_update = fixture
        .request(
            "POST",
            &format!(
                "/emby/Sessions/Capabilities?Id={}&SupportsSync=false",
                fixture.session_id
            ),
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_empty_success(short_update, StatusCode::OK).await;
    let stored = fixture.capabilities().await;
    assert_eq!(stored["PushToken"], "push-final");
    assert_eq!(stored["PushTokenType"], "fcm");
    assert_eq!(stored["SupportsSync"], false);
    assert_eq!(stored["AppId"], "mobile-app");

    // Modern Jellyfin keeps its optional Id fallback and 204 status. Its
    // strongly typed response projection must not expose Emby-only fields.
    let root = fixture
        .request(
            "POST",
            "/Sessions/Capabilities?playableMediaTypes=Video&supportsPersistentIdentifier=true",
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_empty_success(root, StatusCode::NO_CONTENT).await;
    let api = fixture
        .request(
            "POST",
            "/api/Sessions/Capabilities?supportedCommands=Play",
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_empty_success(api, StatusCode::NO_CONTENT).await;
    let api_full = fixture
        .request(
            "POST",
            "/api/Sessions/Capabilities/Full",
            Some(&fixture.token),
            Body::from(
                json!({
                    "PlayableMediaTypes": ["Book"],
                    "PushToken": "must-not-cross-protocols",
                    "SupportsSync": true,
                    "AppId": "must-not-cross-protocols"
                })
                .to_string(),
            ),
        )
        .await;
    assert_empty_success(api_full, StatusCode::NO_CONTENT).await;
    let stored = fixture.capabilities().await;
    assert_eq!(stored["PushToken"], "push-final");
    assert_eq!(stored["PushTokenType"], "fcm");
    assert_eq!(stored["SupportsSync"], false);
    assert_eq!(stored["AppId"], "mobile-app");

    let sessions = fixture.sessions().await;
    let capabilities = &sessions.as_array().unwrap()[0]["Capabilities"];
    for property in ["PushToken", "PushTokenType", "SupportsSync", "AppId"] {
        assert!(
            capabilities.get(property).is_none(),
            "Jellyfin Capabilities leaked {property}: {capabilities}"
        );
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn generated_emby_full_capabilities_rejects_bad_bindings_without_mutation() {
    let fixture = Fixture::new().await;
    let original = fixture.capabilities().await;

    for body in [
        r#"{"PushToken":42}"#,
        r#"{"PushTokenType":[]}"#,
        r#"{"SupportsSync":"true"}"#,
        r#"{"AppId":false}"#,
        "[]",
        "{",
    ] {
        let response = fixture
            .request(
                "POST",
                &format!("/emby/Sessions/Capabilities/Full?Id={}", fixture.session_id),
                Some(&fixture.token),
                Body::from(body),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "body={body}");
        assert_eq!(fixture.capabilities().await, original, "body={body}");
    }

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    row_id: i64,
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
        let user = UserService::new(database.clone())
            .create(&format!("emby-capability-user-{suffix}"))
            .await
            .expect("user creation");
        let device_id = format!("emby-capability-device-{suffix}");
        let device = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Mobile",
                "4.10.0.40",
                "Mobile",
                &device_id,
            ))
            .await
            .expect("device session creation");
        let state = AppState::new(
            database.clone(),
            "Emby Capability Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            database,
            app: jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state)),
            user_id: user.id,
            row_id: device.id,
            token: device.access_token,
            session_id: jellyfin_session_id("Emby Mobile", &device_id),
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

    async fn capabilities(&self) -> Value {
        device::Entity::find_by_id(self.row_id)
            .one(&self.database)
            .await
            .expect("device query")
            .expect("device row")
            .capabilities
    }

    async fn sessions(&self) -> Value {
        let response = self
            .request("GET", "/Sessions", Some(&self.token), Body::empty())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(
            &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn assert_empty_success(response: axum::response::Response, expected: StatusCode) {
    assert_eq!(response.status(), expected);
    assert!(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
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
