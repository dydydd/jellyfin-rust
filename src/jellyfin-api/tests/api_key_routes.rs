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
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"API Key Tests\", DeviceId=\"api-key-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn api_key_routes_match_official_elevated_persisted_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.request("GET", "/Auth/Keys", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request("GET", "/Auth/Keys", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request("POST", "/Auth/Keys", Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let create_response = fixture
        .request(
            "POST",
            &format!("/Auth/Keys?app={}", fixture.created_key_name),
            Some(&fixture.admin_token),
        )
        .await;
    assert_eq!(create_response.status(), StatusCode::NO_CONTENT);

    let keys = body_json(
        fixture
            .request("GET", "/Auth/Keys", Some(&fixture.admin_token))
            .await,
    )
    .await;
    assert_eq!(keys["StartIndex"], 0);
    assert!(
        keys["TotalRecordCount"]
            .as_u64()
            .is_some_and(|count| count >= 2),
        "list must include existing persisted keys: {keys}"
    );
    let created = find_key(&keys, &fixture.created_key_name);
    let created_token = created["AccessToken"]
        .as_str()
        .expect("API key token must be listed");
    assert!(!created_token.is_empty());
    assert_eq!(created["IsActive"], true);
    assert_eq!(created["UserId"], Uuid::nil().simple().to_string());
    assert!(created["DateCreated"].as_str().is_some());
    assert!(created["DateLastActivity"].as_str().is_some());
    assert!(created.get("DateRevoked").is_none());
    assert!(created.get("DeviceId").is_none());

    let api_key_list = body_json(
        fixture
            .request("GET", "/Auth/Keys", Some(&fixture.seed_api_key_token))
            .await,
    )
    .await;
    assert!(find_key_optional(&api_key_list, &fixture.created_key_name).is_some());

    assert_eq!(
        fixture
            .request(
                "DELETE",
                &format!("/Auth/Keys/{created_token}"),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let after_delete = body_json(
        fixture
            .request("GET", "/Auth/Keys", Some(&fixture.admin_token))
            .await,
    )
    .await;
    assert!(find_key_optional(&after_delete, &fixture.created_key_name).is_none());

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    seed_api_key_id: i64,
    seed_api_key_token: String,
    created_key_name: String,
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
            .create_initial_administrator(&format!("api-key-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("api-key-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session_token(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session_token(&devices, user.id, &format!("user-{suffix}")).await;
        let seed_api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("api-key-seed-{suffix}"))
            .await
            .expect("seed API key creation");
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "API Key Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            seed_api_key_id: seed_api_key.id,
            seed_api_key_token: seed_api_key.access_token,
            created_key_name: format!("api-key-created-{suffix}"),
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

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.seed_api_key_id)
            .exec(&self.database)
            .await
            .expect("seed API key cleanup");
        api_key::Entity::delete_many()
            .filter(api_key::Column::Name.eq(self.created_key_name))
            .exec(&self.database)
            .await
            .expect("created API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn session_token(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "API Key Tests",
            "1.0",
            "Test Browser",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
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

fn find_key<'a>(keys: &'a Value, app_name: &str) -> &'a Value {
    find_key_optional(keys, app_name).expect("expected API key must be listed")
}

fn find_key_optional<'a>(keys: &'a Value, app_name: &str) -> Option<&'a Value> {
    keys["Items"]
        .as_array()
        .expect("Items must be an array")
        .iter()
        .find(|key| key["AppName"].as_str() == Some(app_name))
}
