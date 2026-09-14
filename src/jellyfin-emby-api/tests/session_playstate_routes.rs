#![allow(clippy::too_many_lines)]

use std::fmt::Write as _;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice, SessionCommandRepository,
    entities::{api_key, session_command, user},
};
use md5::{Digest, Md5};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Playstate Tests\", DeviceId=\"emby-playstate-tests\", Device=\"Test\", Version=\"1.0\"";

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn generated_emby_playstate_body_is_bound_without_changing_jellyfin_routes() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;

    let mixed_case_route = format!(
        "/emby/sEsSiOnS/{}/pLaYiNg/Seek?SeekPositionTicks=999&ControllingUserId=query",
        fixture.target_session_id
    );
    assert_eq!(
        fixture
            .request(&fixture.emby, &mixed_case_route, None, Body::from("{"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "authentication must precede malformed generated bodies"
    );

    let response = fixture
        .request(
            &fixture.emby,
            &mixed_case_route,
            Some(&fixture.user_token),
            Body::from(
                r#"{
                    "Command":"Pause",
                    "cOmMaNd":"Unpause",
                    "SeekPositionTicks":1,
                    "sEeKpOsItIoNtIcKs":987,
                    "ControllingUserId":"first-controller",
                    "cOnTrOlLiNgUsErId":"body-controller",
                    "UnknownGeneratedExtension":true
                }"#,
            ),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("empty Emby response")
            .is_empty()
    );

    let queued = fixture.queued().await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].message_type, "Playstate");
    assert_eq!(queued[0].payload["Command"], "Seek", "path command wins");
    assert_eq!(queued[0].payload["SeekPositionTicks"], 987);
    assert_eq!(queued[0].payload["ControllingUserId"], "body-controller");

    for body in [Body::empty(), Body::from(r#"{"SeekPositionTicks":"bad"}"#)] {
        assert_eq!(
            fixture
                .request(
                    &fixture.emby,
                    &format!("/emby/Sessions/{}/Playing/Seek", fixture.target_session_id),
                    Some(&fixture.user_token),
                    body,
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(fixture.queued().await.len(), 1);

    fixture
        .devices
        .remove_additional_user(fixture.target_row_id, fixture.user_id)
        .await
        .expect("remove target controller");
    assert_eq!(
        fixture
            .request(
                &fixture.emby,
                &format!("/emby/Sessions/{}/Playing/Pause", fixture.target_session_id),
                Some(&fixture.user_token),
                json_body(&json!({ "Command": "Pause" })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.queued().await.len(), 1);

    let api_key_response = fixture
        .request(
            &fixture.emby,
            &format!("/emby/Sessions/{}/Playing/Pause", fixture.target_session_id),
            Some(&fixture.api_key_token),
            json_body(&json!({
                "Command": "Seek",
                "SeekPositionTicks": 321,
                "ControllingUserId": "api-key-controller"
            })),
        )
        .await;
    assert_eq!(api_key_response.status(), StatusCode::OK);

    for (prefix, ticks, controller) in [("", 111, "root"), ("/api", 222, "api")] {
        let response = fixture
            .request(
                &fixture.jellyfin,
                &format!(
                    "{prefix}/Sessions/{}/Playing/Seek?SeekPositionTicks={ticks}&ControllingUserId={controller}",
                    fixture.target_session_id
                ),
                Some(&fixture.api_key_token),
                json_body(&json!({
                    "Command": "Pause",
                    "SeekPositionTicks": 999,
                    "ControllingUserId": "body-must-not-leak"
                })),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{prefix}");
    }

    let queued = fixture.queued().await;
    assert_eq!(queued.len(), 4);
    assert_eq!(queued[1].payload["Command"], "Pause");
    assert_eq!(queued[1].payload["SeekPositionTicks"], 321);
    assert_eq!(queued[1].payload["ControllingUserId"], "api-key-controller");
    assert_playstate(&queued[2].payload, 111, "root");
    assert_playstate(&queued[3].payload, 222, "api");

    fixture.cleanup().await;
}

fn assert_playstate(payload: &Value, ticks: i64, controller: &str) {
    assert_eq!(payload["Command"], "Seek");
    assert_eq!(payload["SeekPositionTicks"], ticks);
    assert_eq!(payload["ControllingUserId"], controller);
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    devices: DeviceRepository,
    user_id: Uuid,
    other_id: Uuid,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
    target_row_id: i64,
    target_session_id: String,
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
            .create(&format!("emby-playstate-user-{suffix}"))
            .await
            .expect("controller user");
        let other = users
            .create(&format!("emby-playstate-target-{suffix}"))
            .await
            .expect("target user");
        let devices = DeviceRepository::new(database.clone());
        let controller = devices
            .create_session(NewDevice::new(
                user.id,
                "Emby Playstate Controller",
                "1.0",
                "Controller",
                format!("emby-playstate-controller-{suffix}"),
            ))
            .await
            .expect("controller session");
        let target = devices
            .create_session(NewDevice::new(
                other.id,
                "Emby Playstate Target",
                "1.0",
                "Target",
                format!("emby-playstate-target-{suffix}"),
            ))
            .await
            .expect("target session");
        devices
            .add_additional_user(target.id, user.id, &user.username)
            .await
            .expect("share target session with controller");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-playstate-key-{suffix}"))
            .await
            .expect("API key");
        let state = AppState::new(
            database.clone(),
            "Emby Playstate Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );

        Self {
            database,
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            devices,
            user_id: user.id,
            other_id: other.id,
            user_token: controller.access_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            target_row_id: target.id,
            target_session_id: jellyfin_session_id(&target.app_name, &target.device_id),
        }
    }

    async fn request(
        &self,
        app: &Router,
        uri: &str,
        token: Option<&str>,
        body: Body,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        app.clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn queued(&self) -> Vec<session_command::Model> {
        SessionCommandRepository::new(self.database.clone())
            .list_for_session(&self.target_session_id)
            .await
            .expect("queued commands")
    }

    async fn cleanup(self) {
        session_command::Entity::delete_many()
            .filter(session_command::Column::TargetSessionId.eq(self.target_session_id))
            .exec(&self.database)
            .await
            .expect("session command cleanup");
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.user_id, self.other_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

fn json_body(value: &Value) -> Body {
    Body::from(serde_json::to_vec(value).unwrap())
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
