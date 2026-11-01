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
use jellyfin_model::UserPolicy;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Session Provider Tests\", DeviceId=\"session-provider-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn elevated_identities_receive_official_auth_and_password_reset_providers() {
    let fixture = Fixture::new().await;

    for route in ["/Auth/Providers", "/Auth/PasswordResetProviders"] {
        assert_eq!(
            fixture.request(route, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            fixture
                .request(route, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    let response = fixture
        .request("/Auth/Providers", Some(&fixture.admin_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "Name": "Default",
                "Id": UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID
            }
        ])
    );

    let response = fixture
        .request("/Auth/PasswordResetProviders", Some(&fixture.admin_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "Name": "Default Password Reset Provider",
                "Id": UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID
            }
        ])
    );

    let response = fixture
        .request(
            &format!("/Auth/Providers?api_key={}", fixture.api_key_token),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "Name": "Default",
                "Id": UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID
            }
        ])
    );

    let response = fixture
        .request(
            &format!(
                "/Auth/PasswordResetProviders?ApiKey={}",
                fixture.api_key_token
            ),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "Name": "Default Password Reset Provider",
                "Id": UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID
            }
        ])
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
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
        let administrator = users
            .create_initial_administrator(&format!("session-provider-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("session-provider-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("session-provider-key-{suffix}"))
            .await
            .expect("API key creation");
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Session Provider Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            admin_id: administrator.id,
            user_id: user.id,
            admin_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
        }
    }

    async fn request(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
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
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Session Provider Tests",
            "1.0",
            "Test",
            format!("session-provider-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}
