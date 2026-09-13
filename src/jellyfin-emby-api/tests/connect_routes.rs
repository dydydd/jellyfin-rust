use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Connect Tests\", DeviceId=\"emby-connect-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_connect_";

#[tokio::test]
async fn connect_routes_are_truthful_authorized_and_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary PostgreSQL migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_pending().await;
    fixture.assert_exchange().await;
    fixture.assert_link().await;
    fixture.assert_unlink().await;
    fixture.assert_protocol_isolation().await;

    drop(fixture);
    database.close().await.expect("database close");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("connect-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("connect-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("connect-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database.clone(),
            "Emby Connect Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            database,
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            admin_token,
            user_token,
            api_key_token,
        }
    }

    async fn assert_pending(&self) {
        let path = "/emby/cOnNeCt/pEnDiNg";
        assert_eq!(
            request(&self.emby, Method::GET, path, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&self.emby, Method::GET, path, Some(&self.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        for token in [&self.admin_token, &self.api_key_token] {
            let response = request(&self.emby, Method::GET, path, Some(token)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await, json!([]));
        }
    }

    async fn assert_exchange(&self) {
        assert_eq!(
            request(&self.emby, Method::GET, "/emby/Connect/Exchange", None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede required-query binding",
        );
        assert_eq!(
            request(
                &self.emby,
                Method::GET,
                "/emby/Connect/Exchange",
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
        );
        for path in [
            "/emby/cOnNeCt/eXcHaNgE?CONNECTUSERID=first&connectuserid=second",
            "/emby/Connect/Exchange?ConnectUserId=",
        ] {
            let response = request(&self.emby, Method::GET, path, Some(&self.user_token)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(
                response_bytes(response).await.is_empty(),
                "exchange must never fabricate a token"
            );
        }
    }

    async fn assert_link(&self) {
        let malformed = "/emby/Users/not-a-uuid/Connect/Link";
        assert_eq!(
            request(&self.emby, Method::POST, malformed, Some(&self.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&self.emby, Method::POST, malformed, Some(&self.admin_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        let missing = format!("/emby/Users/{}/Connect/Link", Uuid::new_v4());
        assert_eq!(
            request(&self.emby, Method::POST, &missing, Some(&self.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let base = format!("/emby/Users/{}/Connect/Link", self.user_id);
        for path in [base.clone(), format!("{base}?ConnectUsername=%20%20")] {
            assert_eq!(
                request(&self.emby, Method::POST, &path, Some(&self.admin_token))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST,
                "{path}"
            );
        }

        let before = UserService::new(self.database.clone())
            .get(self.user_id)
            .await
            .expect("target before link");
        let mixed = format!(
            "/emby/uSeRs/{}/cOnNeCt/lInK?ConnectUsername=first&cOnNeCtUsErNaMe=second",
            self.user_id
        );
        for token in [&self.admin_token, &self.api_key_token] {
            let response = request(&self.emby, Method::POST, &mixed, Some(token)).await;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                response_json(response).await,
                json!({"Message": "Emby Connect provider is unavailable"})
            );
        }
        let after = UserService::new(self.database.clone())
            .get(self.user_id)
            .await
            .expect("target after link");
        assert_eq!(
            after, before,
            "unavailable Connect must not mutate the user"
        );
    }

    async fn assert_unlink(&self) {
        let canonical = format!("/emby/Users/{}/Connect/Link", self.user_id);
        assert_eq!(
            request(
                &self.emby,
                Method::DELETE,
                &canonical,
                Some(&self.user_token)
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let missing = format!("/emby/Users/{}/Connect/Link", Uuid::new_v4());
        assert_eq!(
            request(
                &self.emby,
                Method::DELETE,
                &missing,
                Some(&self.admin_token)
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );

        let before = UserService::new(self.database.clone())
            .get(self.user_id)
            .await
            .expect("target before unlink");
        let alias = format!("/emby/uSeRs/{}/cOnNeCt/lInK/dElEtE", self.user_id);
        for (method, path, token) in [
            (
                Method::DELETE,
                canonical.as_str(),
                self.admin_token.as_str(),
            ),
            (
                Method::DELETE,
                canonical.as_str(),
                self.api_key_token.as_str(),
            ),
            (Method::POST, alias.as_str(), self.admin_token.as_str()),
            (Method::POST, alias.as_str(), self.api_key_token.as_str()),
        ] {
            let response = request(&self.emby, method, path, Some(token)).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert!(
                response_bytes(response).await.is_empty(),
                "Void response must have no body"
            );
        }
        let after = UserService::new(self.database.clone())
            .get(self.user_id)
            .await
            .expect("target after unlink");
        assert_eq!(
            after, before,
            "idempotent unlink must not mutate unrelated user data"
        );
    }

    async fn assert_protocol_isolation(&self) {
        for path in [
            "/Connect/Pending",
            "/api/Connect/Pending",
            "/Connect/Exchange?ConnectUserId=x",
            "/api/Connect/Exchange?ConnectUserId=x",
            &format!("/Users/{}/Connect/Link?ConnectUsername=x", self.user_id),
            &format!("/api/Users/{}/Connect/Link?ConnectUsername=x", self.user_id),
            &format!("/Users/{}/Connect/Link/Delete", self.user_id),
            &format!("/api/Users/{}/Connect/Link/Delete", self.user_id),
        ] {
            let method = if path.contains("Pending") || path.contains("Exchange") {
                Method::GET
            } else {
                Method::POST
            };
            assert_eq!(
                request(&self.jellyfin, method, path, Some(&self.admin_token))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "Emby Connect leaked into Jellyfin at {path}",
            );
        }
    }
}

async fn request(app: &Router, method: Method, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("Connect request"))
        .await
        .expect("Connect response")
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&response_bytes(response).await).expect("Connect JSON response")
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded Connect response")
        .to_vec()
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Connect Tests",
            "1.0",
            "Test",
            format!("emby-connect-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
