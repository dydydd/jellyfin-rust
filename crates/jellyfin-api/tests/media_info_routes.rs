use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DeviceRepository, NewDevice, entities::user};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Media Info Tests\", DeviceId=\"media-info-tests\", Device=\"Test\", Version=\"1.0\"";
const DEFAULT_SIZE: usize = 102_400;
const MAX_SIZE: usize = 100_000_000;
const REPEATING_BLOCK_SIZE: usize = 4 * 1024;

#[tokio::test]
async fn official_bitrate_test_default_and_valid_size_contract() {
    let fixture = Fixture::new().await;
    for uri in ["/Playback/BitrateTest", "/Playback/BitrateTest?size=102400"] {
        let response = fixture.get(uri, Some(&fixture.admin_token)).await;
        assert_bitrate_headers(&response, DEFAULT_SIZE);
        let body = to_bytes(response.into_body(), DEFAULT_SIZE + 1)
            .await
            .unwrap();
        assert_eq!(body.len(), DEFAULT_SIZE, "{uri}");
        assert!(body[..32].windows(2).any(|bytes| bytes[0] != bytes[1]));
        assert_eq!(
            &body[..DEFAULT_SIZE - REPEATING_BLOCK_SIZE],
            &body[REPEATING_BLOCK_SIZE..],
            "{uri}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn official_bitrate_test_invalid_values_and_parse_boundaries() {
    let fixture = Fixture::new().await;
    for size in [
        "0",
        "-102400",
        "1000000000",
        "100000001",
        "not-a-number",
        "999999999999999999999999999999999999",
    ] {
        let uri = format!("/Playback/BitrateTest?size={size}");
        assert_eq!(
            fixture.get(&uri, Some(&fixture.admin_token)).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn bitrate_test_authentication_and_inclusive_bounds() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.get("/Playback/BitrateTest", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let response = fixture
        .get("/Playback/BitrateTest?size=1", Some(&fixture.user_token))
        .await;
    assert_bitrate_headers(&response, 1);
    assert_eq!(to_bytes(response.into_body(), 2).await.unwrap().len(), 1);

    let response = fixture
        .get(
            "/Playback/BitrateTest?size=100000000",
            Some(&fixture.user_token),
        )
        .await;
    assert_bitrate_headers(&response, MAX_SIZE);
    drop(response);
    fixture.cleanup().await;
}

fn assert_bitrate_headers(response: &axum::response::Response, expected_size: usize) {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        expected_size.to_string()
    );
    assert!(!response.headers().contains_key(header::TRANSFER_ENCODING));
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
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
            .create_initial_administrator(&format!("media-info-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("media-info-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("media-info-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("media-info-user-{suffix}")).await;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Media Info Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().uri(uri);
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
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("media info user cleanup");
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Media Info Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("media info session")
        .access_token
}
