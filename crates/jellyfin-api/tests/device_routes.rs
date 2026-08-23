use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceOptionsRepository, DeviceQuery, DeviceRepository, NewDevice,
    entities::{api_key, device_option, user},
};
use jellyfin_model::UserPolicy;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Device Route Tests\", DeviceId=\"device-route-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn device_routes_match_official_elevated_scope_and_postgres_lifecycle() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.request("GET", "/Devices", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request("GET", "/Devices", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    assert_admin_default_scope(&fixture).await;
    assert_admin_target_user_scope(&fixture).await;
    assert_api_key_global_scope(&fixture).await;
    assert_device_options(&fixture).await;
    assert_device_info_latest_projection(&fixture).await;
    assert_delete_device(&fixture).await;

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    admin_id: Uuid,
    user_id: Uuid,
    other_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
    admin_device_id: String,
    user_device_id: String,
    other_device_id: String,
    option_only_device_id: String,
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
            .create_initial_administrator(&format!("device-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("device-user-{suffix}"))
            .await
            .expect("user creation");
        let other = users
            .create(&format!("device-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());

        let admin_device_id = format!("admin-device-{suffix}");
        let admin_token = session_token(
            &devices,
            admin.id,
            "Admin Client",
            "1.0",
            "Admin Browser",
            &admin_device_id,
        )
        .await;

        let user_device_id = format!("shared-device-{suffix}");
        users
            .update_policy(
                user.id,
                &UserPolicy {
                    enable_all_devices: false,
                    enabled_devices: vec![user_device_id.clone()],
                    authentication_provider_id: Some(
                        UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
                    ),
                    password_reset_provider_id: Some(
                        UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
                    ),
                    ..UserPolicy::default()
                },
            )
            .await
            .expect("restricted device policy");
        create_stale_user_device(&devices, user.id, &user_device_id).await;
        let user_token = session_token(
            &devices,
            user.id,
            "Latest Client",
            "2.0",
            "Latest Browser",
            &user_device_id,
        )
        .await;
        devices
            .update_capabilities_by_token(
                &user_token,
                json!({
                    "PlayableMediaTypes": ["Video"],
                    "SupportedCommands": ["Play"],
                    "SupportsMediaControl": true,
                    "SupportsPersistentIdentifier": true,
                    "IconUrl": "https://example.test/device.png"
                }),
            )
            .await
            .expect("device capabilities update");

        let other_device_id = format!("other-device-{suffix}");
        session_token(
            &devices,
            other.id,
            "Other Client",
            "3.0",
            "Tablet",
            &other_device_id,
        )
        .await;

        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("device-key-{suffix}"))
            .await
            .expect("API key creation");
        let option_only_device_id = format!("option-only-device-{suffix}");

        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Device Route Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            admin_id: admin.id,
            user_id: user.id,
            other_id: other.id,
            admin_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            admin_device_id,
            user_device_id,
            other_device_id,
            option_only_device_id,
        }
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
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

    async fn request_json(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Value,
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
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        device_option::Entity::delete_many()
            .filter(device_option::Column::DeviceId.is_in([
                self.admin_device_id.clone(),
                self.user_device_id.clone(),
                self.other_device_id.clone(),
                self.option_only_device_id.clone(),
            ]))
            .exec(&self.database)
            .await
            .expect("test device options cleanup");
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("test API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id, self.other_id]))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn assert_admin_default_scope(fixture: &Fixture) {
    let devices = body_json(
        fixture
            .request("GET", "/Devices", Some(&fixture.admin_token))
            .await,
    )
    .await;
    assert!(device_count(&devices, &fixture.admin_device_id) >= 1);
    assert_eq!(device_count(&devices, &fixture.user_device_id), 2);
    assert!(device_count(&devices, &fixture.other_device_id) >= 1);
    assert_eq!(devices["StartIndex"], 0);
}

async fn assert_admin_target_user_scope(fixture: &Fixture) {
    let devices = body_json(
        fixture
            .request(
                "GET",
                &format!("/Devices?userId={}", fixture.user_id),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_device_ids(
        &devices,
        &[&fixture.user_device_id, &fixture.user_device_id],
    );
}

async fn assert_api_key_global_scope(fixture: &Fixture) {
    let devices = body_json(
        fixture
            .request("GET", "/Devices", Some(&fixture.api_key_token))
            .await,
    )
    .await;
    assert!(device_count(&devices, &fixture.admin_device_id) >= 1);
    assert_eq!(device_count(&devices, &fixture.user_device_id), 2);
    assert!(device_count(&devices, &fixture.other_device_id) >= 1);
    assert_eq!(devices["StartIndex"], 0);
    assert!(
        devices["TotalRecordCount"]
            .as_u64()
            .is_some_and(|count| count >= 4)
    );
}

async fn assert_device_options(fixture: &Fixture) {
    assert_device_options_access(fixture).await;
    let ghost_options = assert_option_only_device_options_upsert(fixture).await;
    assert_session_device_options_projection(fixture, &ghost_options).await;
}

async fn assert_device_options_access(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request("GET", "/Devices/Options?id=missing-device", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request_json(
                "POST",
                &format!("/Devices/Options?id={}", fixture.user_device_id),
                Some(&fixture.user_token),
                json!({ "CustomName": "Denied" }),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Devices/Options?id={}", fixture.user_device_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request_json(
                "POST",
                "/Devices/Options",
                Some(&fixture.admin_token),
                json!({ "CustomName": "Missing Id" }),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn assert_option_only_device_options_upsert(fixture: &Fixture) -> Value {
    assert_eq!(
        fixture
            .request_json(
                "POST",
                &format!("/Devices/Options?id={}", fixture.option_only_device_id),
                Some(&fixture.admin_token),
                json!({ "CustomName": "Ghost Console" }),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let ghost_options = body_json(
        fixture
            .request(
                "GET",
                &format!("/Devices/Options?id={}", fixture.option_only_device_id),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(ghost_options["DeviceId"], fixture.option_only_device_id);
    assert_eq!(ghost_options["CustomName"], "Ghost Console");
    ghost_options
}

async fn assert_session_device_options_projection(fixture: &Fixture, ghost_options: &Value) {
    assert_eq!(
        fixture
            .request_json(
                "POST",
                &format!("/Devices/Options?id={}", fixture.user_device_id),
                Some(&fixture.admin_token),
                json!({
                    "Id": ghost_options["Id"],
                    "DeviceId": fixture.option_only_device_id,
                    "CustomName": "Family TV"
                }),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let persisted = DeviceOptionsRepository::new(fixture.database.clone())
        .get(&fixture.user_device_id)
        .await
        .expect("persisted device options lookup")
        .expect("persisted device options");
    assert_eq!(persisted.device_id, fixture.user_device_id);
    assert_eq!(persisted.custom_name.as_deref(), Some("Family TV"));

    let devices = body_json(
        fixture
            .request(
                "GET",
                &format!("/Devices?userId={}", fixture.user_id),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    for device in devices["Items"].as_array().expect("Items array") {
        assert_eq!(device["CustomName"], "Family TV");
    }
}

async fn assert_device_info_latest_projection(fixture: &Fixture) {
    let info = body_json(
        fixture
            .request(
                "GET",
                &format!("/Devices/Info?id={}", fixture.user_device_id),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(info["Id"], fixture.user_device_id);
    assert_eq!(info["Name"], "Latest Browser");
    assert_eq!(info["AppName"], "Latest Client");
    assert_eq!(info["AppVersion"], "2.0");
    assert_eq!(info["CustomName"], "Family TV");
    assert_eq!(info["LastUserId"], fixture.user_id.simple().to_string());
    assert!(info["LastUserName"].as_str().is_some());
    assert_eq!(info["Capabilities"]["PlayableMediaTypes"], json!(["Video"]));
    assert_eq!(info["IconUrl"], "https://example.test/device.png");

    assert_eq!(
        fixture
            .request(
                "GET",
                "/Devices/Info?id=missing-device",
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_delete_device(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request(
                "DELETE",
                "/Devices?id=missing-device",
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &format!("/Devices?id={}", fixture.user_device_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let remaining = DeviceRepository::new(fixture.database.clone())
        .query(&DeviceQuery {
            device_id: Some(fixture.user_device_id.clone()),
            ..DeviceQuery::default()
        })
        .await
        .expect("device query after deletion");
    assert_eq!(remaining.total_record_count, 0);
}

async fn session_token(
    devices: &DeviceRepository,
    user_id: Uuid,
    app_name: &str,
    app_version: &str,
    device_name: &str,
    device_id: &str,
) -> String {
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
        .access_token
}

async fn create_stale_user_device(devices: &DeviceRepository, user_id: Uuid, device_id: &str) {
    let mut stale = devices
        .create_session(NewDevice::new(
            user_id,
            "Stale Client",
            "1.0",
            "Stale Browser",
            device_id,
        ))
        .await
        .expect("stale session creation");
    stale.date_last_activity = Utc::now() - Duration::hours(1);
    devices.update(stale).await.expect("stale session update");
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

fn assert_device_ids(devices: &Value, expected: &[&str]) {
    let mut actual = devices["Items"]
        .as_array()
        .expect("Items must be an array")
        .iter()
        .map(|device| {
            device["Id"]
                .as_str()
                .expect("device id must be text")
                .to_owned()
        })
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|device_id| (*device_id).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
}

fn device_count(devices: &Value, device_id: &str) -> usize {
    devices["Items"]
        .as_array()
        .expect("Items must be an array")
        .iter()
        .filter(|device| device["Id"].as_str() == Some(device_id))
        .count()
}
