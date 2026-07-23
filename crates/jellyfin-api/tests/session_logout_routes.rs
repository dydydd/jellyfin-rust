use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Session Logout Tests\", DeviceId=\"session-logout-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn session_logout_matches_official_device_token_and_api_key_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.post("/Sessions/Logout", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get("/Users/Me", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::OK
    );

    let response = fixture
        .post("/Sessions/Logout", Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(body_bytes(response).await.is_empty());
    assert!(
        fixture
            .devices
            .find_by_token(&fixture.user_token)
            .await
            .expect("logged out token lookup must succeed")
            .is_none()
    );
    assert_eq!(
        fixture
            .get("/Users/Me", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let api_key_route = format!("/Sessions/Logout?api_key={}", fixture.api_key_token);
    let response = fixture.post(&api_key_route, None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        fixture
            .api_keys
            .find_by_token(&fixture.api_key_token)
            .await
            .expect("API key lookup must succeed")
            .is_some()
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Auth/Providers?api_key={}", fixture.api_key_token),
                None,
            )
            .await
            .status(),
        StatusCode::OK
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
    devices: DeviceRepository,
    api_keys: ApiKeyRepository,
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
            .create(&format!("session-logout-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "Session Logout Tests",
                "1.0",
                "Test",
                format!("session-logout-tests-{suffix}"),
            ))
            .await
            .expect("session creation")
            .access_token;
        let api_keys = ApiKeyRepository::new(database.clone());
        let api_key = api_keys
            .create(&format!("session-logout-key-{suffix}"))
            .await
            .expect("API key creation");
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Session Logout Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            devices,
            api_keys,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        self.request("GET", uri, token).await
    }

    async fn post(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        self.request("POST", uri, token).await
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

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("test API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .unwrap()
        .to_vec()
}
