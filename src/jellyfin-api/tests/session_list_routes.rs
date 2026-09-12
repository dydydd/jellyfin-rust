use std::fmt::Write as _;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use futures_util::StreamExt;
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

const AUTHORIZATION: &str = "MediaBrowser Client=\"Session List Tests\", DeviceId=\"session-list-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn sessions_list_projects_postgres_device_sessions_with_official_filters() {
    let fixture = Fixture::new().await;

    assert_unauthorized(&fixture).await;
    assert_user_scope(&fixture).await;
    assert_recent_scope(&fixture).await;
    assert_device_filter(&fixture).await;
    assert_elevated_scope(&fixture).await;
    assert_controllable_scope(&fixture).await;
    fixture.cleanup().await;
}

#[tokio::test]
async fn controllable_sessions_follow_association_policy_and_device_access() {
    let fixture = Fixture::new().await;
    let devices = DeviceRepository::new(fixture.database.clone());
    devices
        .update_capabilities_by_token(
            &fixture.other_token,
            json!({
                "PlayableMediaTypes": [],
                "SupportedCommands": [],
                "SupportsMediaControl": true,
                "SupportsPersistentIdentifier": true
            }),
        )
        .await
        .expect("target capabilities update");
    devices
        .add_additional_user(
            fixture.other_device_row_id,
            fixture.user_id,
            &fixture.user_name,
        )
        .await
        .expect("additional user association");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test WebSocket listener");
    let address = listener.local_addr().unwrap();
    let app = fixture.app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/websocket?api_key={}",
        fixture.other_token
    ))
    .await
    .expect("target WebSocket connection");
    socket.next().await.expect("keepalive message").unwrap();
    socket.next().await.expect("sessions message").unwrap();

    let ordinary = body_json(fixture.get("/Sessions", Some(&fixture.user_token)).await).await;
    assert_device_ids(
        &ordinary,
        &[
            &fixture.user_device_id,
            &fixture.stale_user_device_id,
            &fixture.other_device_id,
        ],
    );

    let uri = format!("/Sessions?ControllableByUserId={}", fixture.user_id);
    let associated = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
    assert_device_ids(&associated, &[&fixture.other_device_id]);
    let nil_target = body_json(
        fixture
            .get(
                "/Sessions?ControllableByUserId=00000000-0000-0000-0000-000000000000",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_device_ids(&nil_target, &[&fixture.other_device_id]);

    devices
        .remove_additional_user(fixture.other_device_row_id, fixture.user_id)
        .await
        .expect("additional user removal");
    let unrelated = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
    assert_eq!(unrelated, json!([]));

    let users = UserService::new(fixture.database.clone());
    let stored = users.get(fixture.user_id).await.expect("requesting user");
    let mut policy: jellyfin_model::UserPolicy =
        serde_json::from_value(stored.policy).expect("requesting user policy");
    policy.enable_remote_control_of_other_users = true;
    policy.enable_shared_device_control = false;
    policy.enable_all_devices = false;
    policy.enabled_devices = vec![fixture.other_device_id.to_uppercase()];
    users
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("requesting user policy update");
    let explicitly_allowed = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
    assert_device_ids(&explicitly_allowed, &[&fixture.other_device_id]);

    policy.enabled_devices.clear();
    users
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("device restriction update");
    let persistent_blocked = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
    assert_eq!(persistent_blocked, json!([]));

    devices
        .update_capabilities_by_token(
            &fixture.other_token,
            json!({
                "PlayableMediaTypes": [],
                "SupportedCommands": [],
                "SupportsMediaControl": true,
                "SupportsPersistentIdentifier": false
            }),
        )
        .await
        .expect("non-persistent target capabilities update");
    let non_persistent_allowed =
        body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
    assert_device_ids(&non_persistent_allowed, &[&fixture.other_device_id]);

    socket.close(None).await.unwrap();
    server.abort();
    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    admin_id: Uuid,
    user_id: Uuid,
    other_user_id: Uuid,
    user_name: String,
    admin_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
    admin_device_id: String,
    user_device_id: String,
    stale_user_device_id: String,
    other_device_id: String,
    other_device_row_id: i64,
    other_token: String,
    inactive_device_id: String,
    user_session_id: String,
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
        let (administrator, user, other, user_name) = create_users(&users, &suffix).await;
        let devices = DeviceRepository::new(database.clone());

        let admin_device_id = format!("admin-device-{suffix}");
        let admin_token = create_session_token(
            &devices,
            administrator.id,
            "Admin Client",
            "1.0",
            "Admin Browser",
            &admin_device_id,
        )
        .await;

        let user_device_id = format!("MiXeD-Device-{suffix}");
        let user_session_id = jellyfin_session_id("Jellyfin Web", &user_device_id);
        let user_token = create_session_token(
            &devices,
            user.id,
            "Jellyfin Web",
            "10.10.0",
            "Browser",
            &user_device_id,
        )
        .await;

        let stale_user_device_id = create_stale_session(&devices, user.id, &suffix).await;
        let other_device_id = format!("other-device-{suffix}");
        let other_session = session(
            &devices,
            other.id,
            "Other Client",
            "3.0",
            "Tablet",
            &other_device_id,
        )
        .await;
        let inactive_device_id = create_inactive_session(&devices, user.id, &suffix).await;
        let api_key = create_api_key(&database, &suffix).await;
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Session List Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            admin_id: administrator.id,
            user_id: user.id,
            other_user_id: other.id,
            user_name,
            admin_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            admin_device_id,
            user_device_id,
            stale_user_device_id,
            other_device_id,
            other_device_row_id: other_session.id,
            other_token: other_session.access_token,
            inactive_device_id,
            user_session_id,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Some(token) = token {
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
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("test API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id, self.other_user_id]))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn create_users(
    users: &UserService,
    suffix: &str,
) -> (
    jellyfin_data::entities::user::Model,
    jellyfin_data::entities::user::Model,
    jellyfin_data::entities::user::Model,
    String,
) {
    let administrator = users
        .create_initial_administrator(&format!("session-list-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user_name = format!("session-list-user-{suffix}");
    let user = users.create(&user_name).await.expect("user creation");
    let other = users
        .create(&format!("session-list-other-{suffix}"))
        .await
        .expect("other user creation");
    (administrator, user, other, user_name)
}

async fn create_session_token(
    devices: &DeviceRepository,
    user_id: Uuid,
    app_name: &str,
    app_version: &str,
    device_name: &str,
    device_id: &str,
) -> String {
    session(
        devices,
        user_id,
        app_name,
        app_version,
        device_name,
        device_id,
    )
    .await
    .access_token
}

async fn create_stale_session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    let stale_user_device_id = format!("stale-device-{suffix}");
    let mut stale_session = session(
        devices,
        user_id,
        "Jellyfin Mobile",
        "2.0",
        "Phone",
        &stale_user_device_id,
    )
    .await;
    stale_session.date_last_activity = Utc::now() - Duration::hours(2);
    devices
        .update(stale_session)
        .await
        .expect("stale session timestamp update");
    stale_user_device_id
}

async fn create_inactive_session(
    devices: &DeviceRepository,
    user_id: Uuid,
    suffix: &str,
) -> String {
    let inactive_device_id = format!("inactive-device-{suffix}");
    let mut inactive = session(
        devices,
        user_id,
        "Inactive Client",
        "4.0",
        "Old Device",
        &inactive_device_id,
    )
    .await;
    inactive.is_active = false;
    devices
        .update(inactive)
        .await
        .expect("inactive session update");
    inactive_device_id
}

async fn create_api_key(
    database: &sea_orm::DatabaseConnection,
    suffix: &str,
) -> jellyfin_data::entities::api_key::Model {
    ApiKeyRepository::new(database.clone())
        .create(&format!("session-list-key-{suffix}"))
        .await
        .expect("API key creation")
}

async fn assert_unauthorized(fixture: &Fixture) {
    assert_eq!(
        fixture.get("/Sessions", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

async fn assert_user_scope(fixture: &Fixture) {
    let response = fixture.get("/Sessions", Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let own_sessions = body_json(response).await;
    assert_device_ids(
        &own_sessions,
        &[&fixture.user_device_id, &fixture.stale_user_device_id],
    );
    assert!(!has_device_id(&own_sessions, &fixture.admin_device_id));
    assert!(!has_device_id(&own_sessions, &fixture.other_device_id));
    assert!(!has_device_id(&own_sessions, &fixture.inactive_device_id));
    assert_user_session_projection(&own_sessions, fixture);
}

fn assert_user_session_projection(sessions: &Value, fixture: &Fixture) {
    let user_session = find_by_device_id(sessions, &fixture.user_device_id);
    assert_eq!(user_session["Id"], fixture.user_session_id);
    assert_eq!(user_session["UserId"], fixture.user_id.simple().to_string());
    assert_eq!(user_session["UserName"], fixture.user_name);
    assert_eq!(user_session["Client"], "Jellyfin Web");
    assert_eq!(user_session["DeviceName"], "Browser");
    assert_eq!(user_session["ApplicationVersion"], "10.10.0");
    assert_eq!(user_session["IsActive"], true);
    assert_eq!(user_session["SupportsMediaControl"], false);
    assert_eq!(user_session["SupportsRemoteControl"], false);
    assert_eq!(user_session["HasCustomDeviceName"], false);
    assert_eq!(user_session["PlayableMediaTypes"], json!([]));
    assert_eq!(user_session["SupportedCommands"], json!([]));
    assert_eq!(
        user_session["Capabilities"],
        json!({
            "PlayableMediaTypes": [],
            "SupportedCommands": [],
            "SupportsMediaControl": false,
            "SupportsPersistentIdentifier": true
        })
    );
    assert!(user_session["LastActivityDate"].as_str().is_some());
    assert!(user_session["LastPlaybackCheckIn"].as_str().is_some());
    assert!(
        user_session["ServerId"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(user_session.get("DeviceType").is_none());
    assert!(user_session.get("LastPausedDate").is_none());
}

async fn assert_recent_scope(fixture: &Fixture) {
    for key in [
        "activeWithinSeconds",
        "ActiveWithinSeconds",
        "activewithinseconds",
    ] {
        let recent = body_json(
            fixture
                .get(&format!("/Sessions?{key}=60"), Some(&fixture.user_token))
                .await,
        )
        .await;
        assert_device_ids(&recent, &[&fixture.user_device_id]);
    }

    for value in ["2147483648", "-2147483649"] {
        assert_eq!(
            fixture
                .get(
                    &format!("/Sessions?activeWithinSeconds={value}"),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "activeWithinSeconds uses the SDK/ASP.NET signed Int32 contract"
        );
    }
}

async fn assert_device_filter(fixture: &Fixture) {
    for key in ["deviceId", "DeviceId", "deviceid"] {
        let uri = format!("/Sessions?{key}={}", fixture.user_device_id.to_lowercase());
        let filtered = body_json(fixture.get(&uri, Some(&fixture.admin_token)).await).await;
        assert_device_ids(&filtered, &[&fixture.user_device_id]);
    }
}

async fn assert_elevated_scope(fixture: &Fixture) {
    let admin_sessions =
        body_json(fixture.get("/Sessions", Some(&fixture.admin_token)).await).await;
    assert!(has_device_id(&admin_sessions, &fixture.admin_device_id));
    assert!(has_device_id(&admin_sessions, &fixture.user_device_id));
    assert!(has_device_id(
        &admin_sessions,
        &fixture.stale_user_device_id
    ));
    assert!(has_device_id(&admin_sessions, &fixture.other_device_id));
    assert!(!has_device_id(&admin_sessions, &fixture.inactive_device_id));

    let api_key_sessions = body_json(
        fixture
            .get(
                &format!("/Sessions?api_key={}", fixture.api_key_token),
                None,
            )
            .await,
    )
    .await;
    assert!(has_device_id(&api_key_sessions, &fixture.other_device_id));
    assert!(!has_device_id(
        &api_key_sessions,
        &fixture.inactive_device_id
    ));
}

async fn assert_controllable_scope(fixture: &Fixture) {
    for key in [
        "controllableByUserId",
        "ControllableByUserId",
        "controllablebyuserid",
    ] {
        let controllable = body_json(
            fixture
                .get(
                    &format!("/Sessions?{key}={}", fixture.user_id),
                    Some(&fixture.admin_token),
                )
                .await,
        )
        .await;
        assert_eq!(controllable, json!([]));
    }
}

async fn session(
    devices: &DeviceRepository,
    user_id: Uuid,
    app_name: &str,
    app_version: &str,
    device_name: &str,
    device_id: &str,
) -> device::Model {
    devices
        .create_session(NewDevice::new(
            user_id,
            app_name,
            app_version,
            device_name,
            device_id,
        ))
        .await
        .expect("session creation")
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn assert_device_ids(sessions: &Value, expected: &[&str]) {
    let mut actual = device_ids(sessions);
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected, "sessions={sessions:#?}");
}

fn device_ids(sessions: &Value) -> Vec<String> {
    sessions
        .as_array()
        .expect("sessions must be an array")
        .iter()
        .map(|session| {
            session["DeviceId"]
                .as_str()
                .expect("session device id must be text")
                .to_owned()
        })
        .collect()
}

fn has_device_id(sessions: &Value, device_id: &str) -> bool {
    sessions
        .as_array()
        .expect("sessions must be an array")
        .iter()
        .any(|session| session["DeviceId"].as_str() == Some(device_id))
}

fn find_by_device_id<'a>(sessions: &'a Value, device_id: &str) -> &'a Value {
    sessions
        .as_array()
        .expect("sessions must be an array")
        .iter()
        .find(|session| session["DeviceId"].as_str() == Some(device_id))
        .expect("session with expected device id must exist")
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
