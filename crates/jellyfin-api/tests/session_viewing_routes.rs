use std::fmt::Write as _;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice,
    entities::{base_item, device, user},
};
use md5::{Digest, Md5};
use sea_orm::EntityTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Session Viewing Tests\", DeviceId=\"session-viewing-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn session_viewing_reports_now_viewing_item_into_postgres_session() {
    let fixture = Fixture::new().await;

    assert_viewing_validation(&fixture).await;
    assert_report_viewing_persists_and_projects(&fixture).await;

    fixture.cleanup().await;
}

async fn assert_viewing_validation(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request("POST", "/Sessions/Viewing", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request("POST", "/Sessions/Viewing", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                "/Sessions/Viewing?itemId=not-a-guid",
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
                &format!("/Sessions/Viewing?itemId={}", Uuid::new_v4().simple()),
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
                &format!(
                    "/Sessions/Viewing?sessionId=missing-session&itemId={}",
                    fixture.item_id.simple()
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_report_viewing_persists_and_projects(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/Sessions/Viewing?itemId={}", fixture.item_id.simple()),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let stored = device::Entity::find_by_id(fixture.device_row_id)
        .one(&fixture.database)
        .await
        .expect("device row must load")
        .expect("device row must exist")
        .now_viewing_item
        .expect("now-viewing item must be stored");
    assert_eq!(stored["Name"], "The Matrix");
    assert_eq!(stored["Id"], fixture.item_id.simple().to_string());
    assert_eq!(stored["Type"], "Movie");

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
    assert_eq!(sessions[0]["NowViewingItem"]["Name"], "The Matrix");
    assert_eq!(
        sessions[0]["NowViewingItem"]["ServerId"],
        sessions[0]["ServerId"]
    );
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    item_id: Uuid,
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
        let user = UserService::new(database.clone())
            .create(&format!("session-viewing-user-{suffix}"))
            .await
            .expect("user creation");
        let device_id = format!("session-viewing-device-{suffix}");
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
        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.name = Some("The Matrix".to_owned());
        item.sort_name = Some("Matrix, The".to_owned());
        item.media_type = Some("Video".to_owned());
        BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("base item creation");

        Self {
            app: jellyfin_api::router(AppState::new(
                database.clone(),
                "Session Viewing Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            user_id: user.id,
            item_id,
            user_token: device.access_token,
            device_id: device.device_id.clone(),
            device_row_id: device.id,
            session_id: jellyfin_session_id(&device.app_name, &device.device_id),
        }
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
        base_item::Entity::delete_by_id(self.item_id)
            .exec(&self.database)
            .await
            .expect("base item cleanup");
        user::Entity::delete_by_id(self.user_id)
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
