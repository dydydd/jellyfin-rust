use std::net::SocketAddr;

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DeviceQuery, DeviceRepository, entities::user};
use jellyfin_model::UserPolicy;
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn login_enforces_official_device_session_and_remote_policies() {
    let fixture = Fixture::new().await;

    fixture.set_policy(false, Vec::new(), 0, true).await;
    assert_eq!(
        fixture
            .login("correct-password", "denied-device", None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.device_count().await, 0);

    fixture
        .set_policy(false, vec!["allowed-device".to_owned()], 0, true)
        .await;
    let first = fixture
        .login("correct-password", "allowed-device", None)
        .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_token = response_token(first).await;
    let second = fixture
        .login("correct-password", "allowed-device", None)
        .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_token = response_token(second).await;
    assert!(
        fixture
            .devices
            .find_by_token(&first_token)
            .await
            .expect("old token lookup must succeed")
            .is_none()
    );
    assert!(
        fixture
            .devices
            .find_by_token(&second_token)
            .await
            .expect("new token lookup must succeed")
            .is_some()
    );
    assert_eq!(fixture.device_count().await, 1);

    fixture
        .set_policy(false, vec!["allowed-device".to_owned()], 1, true)
        .await;
    assert_eq!(
        fixture
            .login("correct-password", "other-device", None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.device_count().await, 1);

    fixture
        .set_policy(false, vec!["allowed-device".to_owned()], 0, false)
        .await;
    assert_eq!(
        fixture
            .login("correct-password", "allowed-device", None)
            .await
            .status(),
        StatusCode::OK
    );
    let local_token = fixture
        .login("correct-password", "allowed-device", None)
        .await;
    let local_token = response_token(local_token).await;
    assert_eq!(
        fixture.get_me(&local_token, None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .get_me(&local_token, Some("203.0.113.8:5000"))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .login(
                "correct-password",
                "allowed-device",
                Some("203.0.113.8:5000")
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(fixture.device_count().await, 1);

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: axum::Router,
    users: UserService,
    devices: DeviceRepository,
    user_id: Uuid,
    username: String,
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
        let mut user = users
            .create(&format!("login-policy-user-{suffix}"))
            .await
            .expect("user creation must succeed");
        DefaultAuthenticationProvider::new().change_password(&mut user, "correct-password");
        let user = users
            .set_password_hash(user.id, user.password_hash)
            .await
            .expect("password hash persistence must succeed");
        let app_database = database.clone();
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Login Policy Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            users,
            devices: DeviceRepository::new(app_database),
            user_id: user.id,
            username: user.username,
        }
    }

    async fn set_policy(
        &self,
        enable_all_devices: bool,
        enabled_devices: Vec<String>,
        max_active_sessions: i32,
        enable_remote_access: bool,
    ) {
        let policy = UserPolicy {
            enable_all_devices,
            enabled_devices,
            max_active_sessions,
            enable_remote_access,
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            ..UserPolicy::default()
        };
        self.users
            .update_policy(self.user_id, &policy)
            .await
            .expect("policy update must succeed");
    }

    async fn login(
        &self,
        password: &str,
        device_id: &str,
        remote: Option<&str>,
    ) -> axum::response::Response {
        let authorization = format!(
            "MediaBrowser Client=\"Login Policy Tests\", DeviceId=\"{device_id}\", Device=\"Test\", Version=\"1.0\""
        );
        let mut request = Request::post("/Users/AuthenticateByName")
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "Username": self.username, "Pw": password }).to_string(),
            ))
            .expect("login request must build");
        if let Some(remote) = remote {
            request.extensions_mut().insert(ConnectInfo(
                remote
                    .parse::<SocketAddr>()
                    .expect("remote address must parse"),
            ));
        }
        self.app
            .clone()
            .oneshot(request)
            .await
            .expect("login response")
    }

    async fn get_me(&self, token: &str, remote: Option<&str>) -> axum::response::Response {
        let mut request = Request::get("/Users/Me")
            .header(
                header::AUTHORIZATION,
                format!(
                    "MediaBrowser Client=\"Login Policy Tests\", DeviceId=\"allowed-device\", Device=\"Test\", Version=\"1.0\", Token=\"{token}\""
                ),
            )
            .body(Body::empty())
            .expect("request must build");
        if let Some(remote) = remote {
            request.extensions_mut().insert(ConnectInfo(
                remote
                    .parse::<SocketAddr>()
                    .expect("remote address must parse"),
            ));
        }
        self.app
            .clone()
            .oneshot(request)
            .await
            .expect("request response")
    }

    async fn device_count(&self) -> u64 {
        self.devices
            .query(&DeviceQuery {
                user_id: Some(self.user_id),
                is_active: Some(true),
                ..DeviceQuery::default()
            })
            .await
            .expect("device query must succeed")
            .total_record_count
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("test user cleanup must succeed");
    }
}

async fn response_token(response: axum::response::Response) -> String {
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body must be readable"),
    )
    .expect("response body must be JSON");
    body["AccessToken"]
        .as_str()
        .expect("access token must be present")
        .to_owned()
}
