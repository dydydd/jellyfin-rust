use std::net::SocketAddr;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use jellyfin_networking::{NetworkConfiguration, NetworkManager};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Endpoint Tests\", DeviceId=\"endpoint-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn endpoint_info_matches_official_auth_and_network_contract() {
    let fixture = Fixture::new().await;

    let response = fixture.request(None, None, "/System/Endpoint").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = fixture
        .request(Some(&fixture.user_token), None, "/System/Endpoint")
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = body_json(response).await;
    assert_eq!(body["IsLocal"], true);
    assert_eq!(body["IsInNetwork"], true);
    assert!(body.get("is_local").is_none());

    let response = fixture
        .request(
            Some(&fixture.user_token),
            Some("203.0.113.8:5000"),
            "/System/Endpoint",
        )
        .await;
    assert_eq!(body_json(response).await, endpoint(false, false));

    let response = fixture
        .request(
            Some(&fixture.user_token),
            Some("[::ffff:192.168.1.12]:5000"),
            "/System/Endpoint",
        )
        .await;
    assert_eq!(body_json(response).await, endpoint(false, true));

    let api_key_uri = format!("/System/Endpoint?api_key={}", fixture.api_key_token);
    let response = fixture
        .request(None, Some("10.12.1.20:5000"), &api_key_uri)
        .await;
    assert_eq!(body_json(response).await, endpoint(false, true));

    fixture.cleanup().await;
}

#[tokio::test]
async fn endpoint_info_uses_configured_network_manager() {
    let mut config = NetworkConfiguration::default();
    config.local_network_subnets = vec!["10.0.0.0/8".to_owned(), "!10.0.5.0/24".to_owned()];
    let fixture = Fixture::new()
        .await
        .with_network_manager(NetworkManager::new(config, Vec::new()));

    let response = fixture
        .request(
            Some(&fixture.user_token),
            Some("10.0.6.20:5000"),
            "/System/Endpoint",
        )
        .await;
    assert_eq!(body_json(response).await, endpoint(false, true));

    let response = fixture
        .request(
            Some(&fixture.user_token),
            Some("10.0.5.20:5000"),
            "/System/Endpoint",
        )
        .await;
    assert_eq!(body_json(response).await, endpoint(false, false));

    fixture.cleanup().await;
}

#[tokio::test]
async fn restart_and_shutdown_match_official_local_and_elevated_policy() {
    let fixture = Fixture::new().await;

    let local_restart = fixture
        .request_method(Method::POST, None, None, "/System/Restart")
        .await;
    assert_eq!(local_restart.status(), StatusCode::NO_CONTENT);

    let remote_restart = fixture
        .request_method(
            Method::POST,
            None,
            Some("203.0.113.8:5000"),
            "/System/Restart",
        )
        .await;
    assert_eq!(remote_restart.status(), StatusCode::UNAUTHORIZED);

    let regular_remote_restart = fixture
        .request_method(
            Method::POST,
            Some(&fixture.user_token),
            Some("203.0.113.8:5000"),
            "/System/Restart",
        )
        .await;
    assert_eq!(regular_remote_restart.status(), StatusCode::FORBIDDEN);

    let api_key_restart = fixture
        .request_method(
            Method::POST,
            None,
            Some("203.0.113.8:5000"),
            &format!("/System/Restart?api_key={}", fixture.api_key_token),
        )
        .await;
    assert_eq!(api_key_restart.status(), StatusCode::NO_CONTENT);

    let local_shutdown = fixture
        .request_method(Method::POST, None, None, "/System/Shutdown")
        .await;
    assert_eq!(local_shutdown.status(), StatusCode::UNAUTHORIZED);

    let regular_shutdown = fixture
        .request_method(
            Method::POST,
            Some(&fixture.user_token),
            None,
            "/System/Shutdown",
        )
        .await;
    assert_eq!(regular_shutdown.status(), StatusCode::FORBIDDEN);

    let api_key_shutdown = fixture
        .request_method(
            Method::POST,
            None,
            None,
            &format!("/System/Shutdown?api_key={}", fixture.api_key_token),
        )
        .await;
    assert_eq!(api_key_shutdown.status(), StatusCode::NO_CONTENT);

    fixture.cleanup().await;
}

fn endpoint(is_local: bool, is_in_network: bool) -> Value {
    serde_json::json!({
        "IsLocal": is_local,
        "IsInNetwork": is_in_network
    })
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    api_key_id: i64,
    user_token: String,
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
        let user = UserService::new(database.clone())
            .create(&format!("system-endpoint-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "endpoint tests",
                "1.0.0",
                "test runner",
                format!("system-endpoint-device-{suffix}"),
            ))
            .await
            .unwrap()
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("system-endpoint-key-{suffix}"))
            .await
            .unwrap();
        Self::from_parts(
            database,
            user.id,
            api_key.id,
            user_token,
            api_key.access_token,
            None,
        )
    }

    fn with_network_manager(self, network_manager: NetworkManager) -> Self {
        Self::from_parts(
            self.database,
            self.user_id,
            self.api_key_id,
            self.user_token,
            self.api_key_token,
            Some(network_manager),
        )
    }

    fn from_parts(
        database: sea_orm::DatabaseConnection,
        user_id: Uuid,
        api_key_id: i64,
        user_token: String,
        api_key_token: String,
        network_manager: Option<NetworkManager>,
    ) -> Self {
        let mut state = AppState::new(
            database.clone(),
            "Endpoint Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        if let Some(network_manager) = network_manager {
            state = state.with_network_manager(network_manager);
        }
        Self {
            database,
            app: jellyfin_api::router(state),
            user_id,
            api_key_id,
            user_token,
            api_key_token,
        }
    }

    async fn request(
        &self,
        token: Option<&str>,
        remote_address: Option<&str>,
        uri: &str,
    ) -> axum::response::Response {
        self.request_method(Method::GET, token, remote_address, uri)
            .await
    }

    async fn request_method(
        &self,
        method: Method,
        token: Option<&str>,
        remote_address: Option<&str>,
        uri: &str,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        let mut request = builder.body(Body::empty()).unwrap();
        if let Some(remote_address) = remote_address {
            request.extensions_mut().insert(ConnectInfo(
                remote_address
                    .parse::<SocketAddr>()
                    .expect("test remote address must parse"),
            ));
        }
        self.app.clone().oneshot(request).await.unwrap()
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .unwrap();
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .unwrap();
    }
}
