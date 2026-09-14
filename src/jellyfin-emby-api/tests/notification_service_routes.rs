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
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Notification Tests\", DeviceId=\"emby-notification-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_notifications_";

#[tokio::test]
async fn notification_service_routes_match_official_empty_provider_behavior() {
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
        panic!("temporary database test task was cancelled: {error}");
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
    fixture.assert_admin_notification().await;
    fixture.assert_test_notification().await;
    fixture.assert_default_notification_info().await;
    fixture.assert_protocol_isolation().await;

    drop(fixture);
    database.close().await.expect("database close");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    user_id: Uuid,
    user_token: String,
    admin_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("notification-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("notification-user-{suffix}"))
            .await
            .expect("ordinary user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("notification-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Notification Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            user_token,
            admin_token,
            api_key_token,
        }
    }

    async fn assert_admin_notification(&self) {
        let malformed = RequestSpec::json(
            Method::POST,
            "/emby/Notifications/Admin?Name=ignored&Description=ignored",
            "{",
        );
        assert_eq!(
            request(&self.emby, &malformed, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede JSON binding",
        );
        assert_eq!(
            request(&self.emby, &malformed, Some(&self.user_token))
                .await
                .status(),
            StatusCode::NO_CONTENT,
            "the legacy binder defaults a malformed body",
        );

        let requests = [
            RequestSpec::empty(Method::POST, "/emby/Notifications/Admin"),
            RequestSpec::json(
                Method::POST,
                "/emby/nOtIfIcAtIoNs/aDmIn?Name=first&nAmE=last&Description=first&dEsCrIpTiOn=last&ImageUrl=one&iMaGeUrL=two&Url=one&uRl=two&Level=Normal&lEvEl=Warning",
                r#"{"DisplayDateTime":true,"dIsPlAyDaTeTiMe":false,"Unknown":1}"#,
            ),
        ];
        for request_spec in &requests {
            for token in [&self.user_token, &self.admin_token, &self.api_key_token] {
                let response = request(&self.emby, request_spec, Some(token)).await;
                assert_eq!(response.status(), StatusCode::NO_CONTENT);
                assert!(response_bytes(response).await.is_empty());
            }
        }
    }

    async fn assert_test_notification(&self) {
        let malformed = RequestSpec::json(
            Method::POST,
            "/emby/Notifications/Services/Test",
            r#"{"Enabled":"true"}"#,
        );
        assert_eq!(
            request(&self.emby, &malformed, None).await.status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            request(&self.emby, &malformed, Some(&self.user_token))
                .await
                .status(),
            StatusCode::NO_CONTENT,
            "the legacy binder defaults an incompatible body",
        );

        let requests = [
            RequestSpec::empty(Method::POST, "/emby/Notifications/Services/Test"),
            RequestSpec::json(Method::POST, "/emby/nOtIfIcAtIoNs/sErViCeS/tEsT", "{}"),
            RequestSpec::json(
                Method::POST,
                "/emby/Notifications/Services/Test",
                r#"{"NotifierKey":"first","nOtIfIeRkEy":"missing","Enabled":true,"UserIds":[],"DeviceIds":[],"LibraryIds":[],"EventIds":[],"Options":{"Url":"https://example.invalid"},"Unknown":null}"#,
            ),
        ];
        for request_spec in &requests {
            for token in [&self.user_token, &self.api_key_token] {
                let response = request(&self.emby, request_spec, Some(token)).await;
                assert_eq!(response.status(), StatusCode::NO_CONTENT);
                assert!(response_bytes(response).await.is_empty());
            }
        }
    }

    async fn assert_default_notification_info(&self) {
        let missing = RequestSpec::empty(
            Method::GET,
            "/emby/Notifications/Services/Defaults?UserId=not-a-guid",
        );
        assert_eq!(
            request(&self.emby, &missing, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede hidden-query binding",
        );

        let cases = [
            (
                "/emby/Notifications/Services/Defaults".to_owned(),
                "Object reference not set to an instance of an object.",
            ),
            (
                "/emby/Notifications/Services/Defaults?UserId=not-a-guid".to_owned(),
                "Guid should contain 32 digits with 4 dashes (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx).",
            ),
            (
                format!(
                    "/emby/Notifications/Services/Defaults?UserId={}&NotifierKey=missing",
                    Uuid::new_v4()
                ),
                "Object reference not set to an instance of an object.",
            ),
            (
                format!(
                    "/emby/Notifications/Services/Defaults?UserId={}",
                    self.user_id
                ),
                "Sequence contains no matching element",
            ),
            (
                format!(
                    "/emby/nOtIfIcAtIoNs/sErViCeS/dEfAuLtS?UserId=not-a-guid&uSeRiD={}&NotifierKey=first&nOtIfIeRkEy=last",
                    self.user_id
                ),
                "Sequence contains no matching element",
            ),
        ];
        for (path, expected_body) in cases {
            for token in [&self.user_token, &self.api_key_token] {
                let response = request(
                    &self.emby,
                    &RequestSpec::empty(Method::GET, &path),
                    Some(token),
                )
                .await;
                assert_eq!(
                    response.status(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "{path}"
                );
                assert_eq!(response_text(response).await, expected_body, "{path}");
            }
        }
    }

    async fn assert_protocol_isolation(&self) {
        for (method, path) in [
            (Method::POST, "/Notifications/Admin"),
            (Method::POST, "/api/Notifications/Admin"),
            (Method::POST, "/Notifications/Services/Test"),
            (Method::POST, "/api/Notifications/Services/Test"),
            (Method::GET, "/Notifications/Services/Defaults"),
            (Method::GET, "/api/Notifications/Services/Defaults"),
        ] {
            let response = request(
                &self.jellyfin,
                &RequestSpec::empty(method, path),
                Some(&self.admin_token),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}

struct RequestSpec<'a> {
    method: Method,
    uri: &'a str,
    body: Option<&'a str>,
}

impl<'a> RequestSpec<'a> {
    fn empty(method: Method, uri: &'a str) -> Self {
        Self {
            method,
            uri,
            body: None,
        }
    }

    fn json(method: Method, uri: &'a str, body: &'a str) -> Self {
        Self {
            method,
            uri,
            body: Some(body),
        }
    }
}

async fn request(app: &Router, spec: &RequestSpec<'_>, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(spec.method.clone()).uri(spec.uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if spec.body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(spec.body.unwrap_or_default().to_owned()))
                .expect("notification request"),
        )
        .await
        .expect("notification route response")
}

async fn response_bytes(response: Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded notification response")
}

async fn response_text(response: Response) -> String {
    String::from_utf8(response_bytes(response).await.to_vec()).expect("UTF-8 notification response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Notification Tests",
            "1.0",
            "Test",
            format!("emby-notification-tests-{suffix}"),
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
