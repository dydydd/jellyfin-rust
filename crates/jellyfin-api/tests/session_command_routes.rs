use axum::{
    Router,
    body::Body,
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
use std::fmt::Write as _;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Session Command Tests\", DeviceId=\"session-command-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn session_command_routes_queue_official_commands_in_postgres() {
    let fixture = Fixture::new().await;

    assert_command_access_and_validation(&fixture).await;
    assert_play_command_validation(&fixture).await;
    assert_playstate_command_validation(&fixture).await;
    enqueue_official_session_commands(&fixture).await;
    assert_queued_commands(&fixture).await;

    fixture.cleanup().await;
}

async fn assert_command_access_and_validation(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request("POST", &fixture.command_uri("GoHome"), None, Body::empty())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &fixture.command_uri("GoHome"),
                Some(&fixture.api_key_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                "/Sessions/missing-session/Command/GoHome",
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &fixture.command_uri("TotallyInvalid"),
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!(
                    "/Sessions/{}/Viewing?itemType=Movie",
                    fixture.target_session_id
                ),
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn assert_play_command_validation(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/Sessions/{}/Playing?itemIds=", fixture.target_session_id),
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!(
                    "/Sessions/{}/Playing?playCommand=PlayNow",
                    fixture.target_session_id
                ),
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!(
                    "/Sessions/{}/Playing?playCommand=TotallyInvalid&itemIds={}",
                    fixture.target_session_id,
                    Uuid::new_v4().simple()
                ),
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn assert_playstate_command_validation(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!(
                    "/Sessions/{}/Playing/TotallyInvalid",
                    fixture.target_session_id
                ),
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                "/Sessions/missing-session/Playing/Pause",
                Some(&fixture.user_token),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn enqueue_official_session_commands(fixture: &Fixture) {
    fixture.post_command("Command/GoHome", Body::empty()).await;
    fixture.post_command("System/Mute", Body::empty()).await;
    fixture
        .post_command(
            &format!(
                "Viewing?itemType=Movie&itemId={}&itemName=The%20Matrix",
                Uuid::new_v4()
            ),
            Body::empty(),
        )
        .await;
    fixture
        .post_command(
            &format!(
                "Playing?playCommand=PlayNow&itemIds={},{}&startPositionTicks=123&mediaSourceId=source-1&audioStreamIndex=2&subtitleStreamIndex=3&startIndex=1",
                play_item_ids()[0].simple(),
                play_item_ids()[1].simple()
            ),
            Body::empty(),
        )
        .await;
    fixture
        .post_command(
            "Playing/Seek?seekPositionTicks=987&controllingUserId=ignored",
            Body::empty(),
        )
        .await;
    fixture
        .post_command(
            "Command",
            json_body(&json!({
                "Name": "SetVolume",
                "ControllingUserId": Uuid::nil().simple().to_string(),
                "Arguments": {
                    "Volume": "50"
                }
            })),
        )
        .await;
    fixture
        .post_command(
            "Message",
            json_body(&json!({
                "Header": "   ",
                "Text": "Hello remote client",
                "TimeoutMs": 1500
            })),
        )
        .await;
}

async fn assert_queued_commands(fixture: &Fixture) {
    let queued = SessionCommandRepository::new(fixture.database.clone())
        .list_for_session(&fixture.target_session_id)
        .await
        .expect("queued commands must load");
    assert_eq!(queued.len(), 7);
    assert!(queued.iter().all(|command| {
        command.target_session_id == fixture.target_session_id
            && command.controlling_session_id.as_deref() == Some(&fixture.controller_session_id)
    }));
    assert_eq!(
        queued
            .iter()
            .map(|command| command.message_type.as_str())
            .collect::<Vec<_>>(),
        [
            "GeneralCommand",
            "GeneralCommand",
            "GeneralCommand",
            "Play",
            "Playstate",
            "GeneralCommand",
            "GeneralCommand"
        ]
    );
    assert_eq!(queued[0].payload["Name"], "GoHome");
    assert_eq!(queued[1].payload["Name"], "Mute");
    assert_eq!(queued[2].payload["Name"], "DisplayContent");
    assert_eq!(queued[2].payload["Arguments"]["ItemType"], "Movie");
    assert_eq!(queued[2].payload["Arguments"]["ItemName"], "The Matrix");
    assert!(queued[2].payload["Arguments"]["ItemId"].as_str().is_some());
    assert_queued_play_command(&queued[3], fixture.user_id);
    assert_queued_playstate_command(&queued[4], fixture.user_id);
    assert_eq!(queued[5].payload["Name"], "SetVolume");
    assert_eq!(queued[5].payload["Arguments"]["Volume"], "50");
    assert_eq!(
        queued[5].payload["ControllingUserId"],
        fixture.user_id.simple().to_string()
    );
    assert_eq!(queued[6].payload["Name"], "DisplayMessage");
    assert_eq!(
        queued[6].payload["Arguments"]["Header"],
        "Message from Server"
    );
    assert_eq!(
        queued[6].payload["Arguments"]["Text"],
        "Hello remote client"
    );
    assert_eq!(queued[6].payload["Arguments"]["TimeoutMs"], "1500");
}

fn assert_queued_play_command(command: &session_command::Model, controlling_user_id: Uuid) {
    let item_ids = play_item_ids();
    assert_eq!(command.payload["PlayCommand"], "PlayNow");
    assert_eq!(
        command.payload["ItemIds"],
        json!([
            item_ids[0].simple().to_string(),
            item_ids[1].simple().to_string()
        ])
    );
    assert_eq!(
        command.payload["ControllingUserId"],
        controlling_user_id.simple().to_string()
    );
    assert_eq!(command.payload["StartPositionTicks"], 123);
    assert_eq!(command.payload["MediaSourceId"], "source-1");
    assert_eq!(command.payload["AudioStreamIndex"], 2);
    assert_eq!(command.payload["SubtitleStreamIndex"], 3);
    assert_eq!(command.payload["StartIndex"], 1);
}

fn assert_queued_playstate_command(command: &session_command::Model, controlling_user_id: Uuid) {
    assert_eq!(command.payload["Command"], "Seek");
    assert_eq!(command.payload["SeekPositionTicks"], 987);
    assert_eq!(
        command.payload["ControllingUserId"],
        controlling_user_id.simple().to_string()
    );
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_id: Uuid,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
    controller_session_id: String,
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
            .create(&format!("session-command-user-{suffix}"))
            .await
            .expect("user creation");
        let other = users
            .create(&format!("session-command-target-{suffix}"))
            .await
            .expect("target user creation");
        let devices = DeviceRepository::new(database.clone());
        let controller = devices
            .create_session(NewDevice::new(
                user.id,
                "Controller Client",
                "1.0",
                "Controller",
                format!("controller-{suffix}"),
            ))
            .await
            .expect("controller session creation");
        let target = devices
            .create_session(NewDevice::new(
                other.id,
                "Target Client",
                "2.0",
                "Target",
                format!("target-{suffix}"),
            ))
            .await
            .expect("target session creation");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("session-command-key-{suffix}"))
            .await
            .expect("API key creation");
        let user_token = controller.access_token.clone();
        let controller_session_id =
            jellyfin_session_id(&controller.app_name, &controller.device_id);
        let target_session_id = jellyfin_session_id(&target.app_name, &target.device_id);

        Self {
            app: jellyfin_api::router(AppState::new(
                database.clone(),
                "Session Command Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            user_id: user.id,
            other_id: other.id,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            controller_session_id,
            target_session_id,
        }
    }

    fn command_uri(&self, command: &str) -> String {
        format!("/Sessions/{}/Command/{command}", self.target_session_id)
    }

    async fn post_command(&self, suffix: &str, body: Body) {
        assert_eq!(
            self.request(
                "POST",
                &format!("/Sessions/{}/{suffix}", self.target_session_id),
                Some(&self.user_token),
                body,
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
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

fn play_item_ids() -> [Uuid; 2] {
    [
        Uuid::parse_str("f9c1ad0c-820f-44df-8db8-52fbfc0d3d93").unwrap(),
        Uuid::parse_str("2b8cf5ff-3f3d-4f7f-a452-6a7f8d190cce").unwrap(),
    ]
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
