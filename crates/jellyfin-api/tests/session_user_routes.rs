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

const AUTHORIZATION: &str = "MediaBrowser Client=\"Session User Tests\", DeviceId=\"session-user-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn session_user_routes_manage_additional_users_in_postgres() {
    let fixture = Fixture::new().await;

    assert_additional_user_validation(&fixture).await;
    assert_add_and_remove_additional_user(&fixture).await;

    fixture.cleanup().await;
}

async fn assert_additional_user_validation(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request("POST", &fixture.user_uri(fixture.additional_user_id), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/Sessions/{}/User/not-a-guid", fixture.session_id),
                Some(&fixture.user_token),
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
                    "/Sessions/missing-session/User/{}",
                    fixture.additional_user_id.simple()
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &fixture.user_uri(fixture.user_id),
                Some(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &fixture.user_uri(Uuid::new_v4()),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_add_and_remove_additional_user(fixture: &Fixture) {
    for _ in 0..2 {
        assert_eq!(
            fixture
                .request(
                    "POST",
                    &fixture.user_uri(fixture.additional_user_id),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        additional_users(fixture).await,
        json!([{
            "UserId": fixture.additional_user_id.simple().to_string(),
            "UserName": fixture.additional_user_name
        }])
    );

    for _ in 0..2 {
        assert_eq!(
            fixture
                .request(
                    "DELETE",
                    &fixture.user_uri(fixture.additional_user_id),
                    Some(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(additional_users(fixture).await, json!([]));

    assert_eq!(
        fixture
            .request(
                "DELETE",
                &fixture.user_uri(fixture.user_id),
                Some(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn additional_users(fixture: &Fixture) -> Value {
    let sessions = body_json(
        fixture
            .request(
                "GET",
                &format!("/Sessions?deviceId={}", fixture.device_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(sessions.as_array().expect("sessions array").len(), 1);
    assert_eq!(sessions[0]["Id"], fixture.session_id);

    let persisted = device::Entity::find_by_id(fixture.device_row_id)
        .one(&fixture.database)
        .await
        .expect("session device must load")
        .expect("session device must exist")
        .additional_users;
    assert_eq!(sessions[0]["AdditionalUsers"], persisted);
    persisted
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    additional_user_id: Uuid,
    additional_user_name: String,
    user_token: String,
    device_id: String,
    device_row_id: i64,
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
            .create(&format!("session-user-primary-{suffix}"))
            .await
            .expect("primary user creation");
        let additional_user_name = format!("session-user-additional-{suffix}");
        let additional_user = users
            .create(&additional_user_name)
            .await
            .expect("additional user creation");
        let device_id = format!("session-user-device-{suffix}");
        let device = DeviceRepository::new(database.clone())
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
            app: jellyfin_api::router(AppState::new(
                database.clone(),
                "Session User Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            user_id: user.id,
            additional_user_id: additional_user.id,
            additional_user_name,
            user_token: device.access_token,
            device_id: device.device_id.clone(),
            device_row_id: device.id,
            session_id: jellyfin_session_id(&device.app_name, &device.device_id),
        }
    }

    fn user_uri(&self, user_id: Uuid) -> String {
        format!("/Sessions/{}/User/{}", self.session_id, user_id.simple())
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
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
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.user_id, self.additional_user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
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
