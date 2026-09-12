use std::fmt::Write as _;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, device, user},
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

    for query in [
        format!(
            "id={}&playableMediaTypes=Video,Audio&supportedCommands=Play,DisplayMessage&supportsMediaControl=true&supportsPersistentIdentifier=false",
            fixture.session_id
        ),
        format!(
            "Id={}&PlayableMediaTypes=Video,Audio&SupportedCommands=Play,DisplayMessage&SupportsMediaControl=true&SupportsPersistentIdentifier=false",
            fixture.session_id
        ),
        format!(
            "id={}&playablemediatypes=Video,Audio&supportedcommands=Play,DisplayMessage&supportsmediacontrol=true&supportspersistentidentifier=false",
            fixture.session_id
        ),
    ] {
        let query_response = fixture
            .request(
                "POST",
                &format!("/Sessions/Capabilities?{query}"),
                Some(&fixture.token),
                Body::empty(),
            )
            .await;
        assert_eq!(query_response.status(), StatusCode::NO_CONTENT);
        assert_query_capabilities(&fixture.sessions().await);
    }

    let full_response = fixture
        .request(
            "POST",
            &format!(
                "/Sessions/Capabilities/Full?id={}&supportsMediaControl=not-a-bool",
                fixture.session_id
            ),
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

    let mixed_case_response = fixture
        .request(
            "POST",
            &format!("/sessions/capabilities/full?id={}", fixture.session_id),
            Some(&fixture.token),
            Body::from(
                json!({
                    "playableMediaTypes": ["video", 2, "3"],
                    "supportedCommands": ["play", 41, "42"],
                    "supportsMediaControl": true,
                    "supportsPersistentIdentifier": false,
                    "deviceProfile": {
                        "name": "Mobile profile",
                        "maxStreamingBitrate": "456789",
                        "directPlayProfiles": [{
                            "container": "mp4",
                            "type": "video",
                            "UnknownNestedProperty": true
                        }],
                        "UnknownProfileProperty": true
                    }
                })
                .to_string(),
            ),
        )
        .await;
    assert_eq!(mixed_case_response.status(), StatusCode::NO_CONTENT);
    let mixed_case = fixture.sessions().await;
    let capabilities = &only_session(&mixed_case)["Capabilities"];
    assert_eq!(
        capabilities["PlayableMediaTypes"],
        json!(["Video", "Audio", "Photo"])
    );
    assert_eq!(
        capabilities["SupportedCommands"],
        json!(["Play", "SetMaxStreamingBitrate", "SetPlaybackOrder"])
    );
    assert_eq!(capabilities["DeviceProfile"]["Name"], "Mobile profile");
    assert_eq!(
        capabilities["DeviceProfile"]["MaxStreamingBitrate"],
        456_789
    );
    assert_eq!(
        capabilities["DeviceProfile"]["DirectPlayProfiles"][0]["Type"],
        "Video"
    );
    assert!(
        capabilities["DeviceProfile"]
            .get("UnknownProfileProperty")
            .is_none()
    );
    assert!(
        capabilities["DeviceProfile"]["DirectPlayProfiles"][0]
            .get("UnknownNestedProperty")
            .is_none()
    );

    let delimited_response = fixture
        .request(
            "POST",
            &format!("/Sessions/Capabilities/Full?id={}", fixture.session_id),
            Some(&fixture.token),
            Body::from(
                json!({
                    "PlayableMediaTypes": "video,2,Book,invalid",
                    "SupportedCommands": "play,41,GoHome,invalid"
                })
                .to_string(),
            ),
        )
        .await;
    assert_eq!(delimited_response.status(), StatusCode::NO_CONTENT);
    let delimited = fixture.sessions().await;
    let capabilities = &only_session(&delimited)["Capabilities"];
    assert_eq!(
        capabilities["PlayableMediaTypes"],
        json!(["Video", "Audio", "Book"])
    );
    assert_eq!(
        capabilities["SupportedCommands"],
        json!(["Play", "SetMaxStreamingBitrate", "GoHome"])
    );

    for invalid_profile in [json!(42), json!([]), json!("profile")] {
        let invalid_profile_response = fixture
            .request(
                "POST",
                &format!("/Sessions/Capabilities/Full?id={}", fixture.session_id),
                Some(&fixture.token),
                Body::from(json!({ "DeviceProfile": invalid_profile }).to_string()),
            )
            .await;
        assert_eq!(invalid_profile_response.status(), StatusCode::BAD_REQUEST);
    }

    for key in ["id", "Id"] {
        let invalid_id_response = fixture
            .request(
                "POST",
                &format!("/Sessions/Capabilities?{key}=not-this-session"),
                Some(&fixture.token),
                Body::empty(),
            )
            .await;
        assert_eq!(invalid_id_response.status(), StatusCode::NOT_FOUND);
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn directed_capabilities_require_control_and_update_the_target_session() {
    let fixture = Fixture::new().await;
    let original = fixture.target_capabilities().await;

    let forbidden = fixture
        .request(
            "POST",
            &format!(
                "/Sessions/Capabilities?id={}&supportsMediaControl=true",
                fixture.target_session_id
            ),
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(fixture.target_capabilities().await, original);

    let api_key_target = fixture
        .request(
            "POST",
            &format!(
                "/Sessions/Capabilities/Full?id={}",
                fixture.target_session_id
            ),
            Some(&fixture.api_key_token),
            Body::from(
                json!({
                    "PlayableMediaTypes": ["Book"],
                    "SupportsMediaControl": true
                })
                .to_string(),
            ),
        )
        .await;
    assert_eq!(api_key_target.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fixture.target_capabilities().await["PlayableMediaTypes"],
        json!(["Book"])
    );

    let api_key_self = fixture
        .request_with_authorization(
            "POST",
            "/Sessions/Capabilities/Full",
            &format!(
                "MediaBrowser Client=\"Target Client\", DeviceId=\"{}\", Device=\"Target Device\", Version=\"1.0\"",
                fixture.target_device_id
            ),
            Some(&fixture.api_key_token),
            Body::from(
                json!({
                    "PlayableMediaTypes": ["Photo"],
                    "SupportsMediaControl": false
                })
                .to_string(),
            ),
        )
        .await;
    assert_eq!(api_key_self.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fixture.target_capabilities().await["PlayableMediaTypes"],
        json!(["Photo"])
    );

    let devices = DeviceRepository::new(fixture.database.clone());
    devices
        .add_additional_user(fixture.target_row_id, fixture.user_id, "controller")
        .await
        .expect("target additional user update");
    let associated = fixture
        .request(
            "POST",
            &format!(
                "/Sessions/Capabilities/Full?id={}",
                fixture.target_session_id
            ),
            Some(&fixture.token),
            Body::from(
                json!({
                    "PlayableMediaTypes": ["Video"],
                    "SupportsMediaControl": true
                })
                .to_string(),
            ),
        )
        .await;
    assert_eq!(associated.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fixture.target_capabilities().await["PlayableMediaTypes"],
        json!(["Video"])
    );

    devices
        .remove_additional_user(fixture.target_row_id, fixture.user_id)
        .await
        .expect("target additional user removal");
    let users = UserService::new(fixture.database.clone());
    let stored = users.get(fixture.user_id).await.expect("controller user");
    let mut policy: jellyfin_model::UserPolicy =
        serde_json::from_value(stored.policy).expect("controller policy");
    policy.enable_remote_control_of_other_users = true;
    users
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("remote-control policy update");
    let permitted = fixture
        .request(
            "POST",
            &format!(
                "/Sessions/Capabilities?id={}&playableMediaTypes=Audio",
                fixture.target_session_id
            ),
            Some(&fixture.token),
            Body::empty(),
        )
        .await;
    assert_eq!(permitted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        fixture.target_capabilities().await["PlayableMediaTypes"],
        json!(["Audio"])
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    target_user_id: Uuid,
    token: String,
    api_key_id: i64,
    api_key_token: String,
    session_id: String,
    target_session_id: String,
    target_device_id: String,
    target_row_id: i64,
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
        let target_user = users
            .create(&format!("capabilities-target-{suffix}"))
            .await
            .expect("target user creation");
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
        let target_device_id = format!("capabilities-target-device-{suffix}");
        let target = devices
            .create_session(NewDevice::new(
                target_user.id,
                "Target Client",
                "1.0",
                "Target Device",
                &target_device_id,
            ))
            .await
            .expect("target session creation");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("session-capabilities-key-{suffix}"))
            .await
            .expect("API key creation");
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Session Capability Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            target_user_id: target_user.id,
            token: session.access_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            session_id: jellyfin_session_id("Jellyfin Web", &device_id),
            target_session_id: jellyfin_session_id("Target Client", &target_device_id),
            target_device_id,
            target_row_id: target.id,
        }
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Body,
    ) -> axum::response::Response {
        self.request_with_authorization(method, uri, AUTHORIZATION, token, body)
            .await
    }

    async fn request_with_authorization(
        &self,
        method: &str,
        uri: &str,
        authorization: &str,
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
                format!("{authorization}, Token=\"{token}\""),
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

    async fn target_capabilities(&self) -> Value {
        device::Entity::find_by_id(self.target_row_id)
            .one(&self.database)
            .await
            .expect("target session query")
            .expect("target session")
            .capabilities
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.user_id, self.target_user_id]))
            .exec(&self.database)
            .await
            .expect("test users cleanup");
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
