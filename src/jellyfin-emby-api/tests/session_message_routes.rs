use std::fmt::Write as _;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, SessionCommandRepository,
};
use md5::{Digest, Md5};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Session Message Tests\", DeviceId=\"emby-session-message-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_session_message_";

#[tokio::test]
async fn session_message_uses_generated_query_contract_only_below_emby() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator cleanup");
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
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");
    let fixture = Fixture::new(database.clone()).await;

    fixture.assert_authentication_and_binding().await;
    fixture.assert_generated_query_request().await;
    fixture.assert_api_key_request().await;
    fixture.assert_jellyfin_body_contract().await;

    database.close().await.expect("database cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    user_token: String,
    api_key: String,
    target_session_id: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let user = UserService::new(database.clone())
            .create(&format!("session-message-user-{suffix}"))
            .await
            .expect("ordinary user");
        let devices = DeviceRepository::new(database.clone());
        let controller = devices
            .create_session(NewDevice::new(
                user.id,
                "Emby Session Message Controller",
                "1.0",
                "Controller",
                format!("controller-{suffix}"),
            ))
            .await
            .expect("controller session");
        let target = devices
            .create_session(NewDevice::new(
                user.id,
                "Emby Session Message Target",
                "1.0",
                "Target",
                format!("target-{suffix}"),
            ))
            .await
            .expect("target session");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("session-message-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let target_session_id = jellyfin_session_id(&target.app_name, &target.device_id);
        let state = AppState::new(
            database.clone(),
            "Emby Session Message Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            database,
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_token: controller.access_token,
            api_key,
            target_session_id,
        }
    }

    async fn assert_authentication_and_binding(&self) {
        let base = format!("/emby/Sessions/{}/Message", self.target_session_id);
        assert_eq!(
            request(&self.emby, Method::POST, &base, None, Body::from("{"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede Emby query binding",
        );
        for query in [
            "Header=Notice",
            "Text=Hello",
            "Text=Hello&Header=Notice&TimeoutMs=invalid",
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    &format!("{base}?{query}"),
                    Some(&self.user_token),
                    Body::empty(),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "invalid generated query {query}",
            );
        }
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &format!("/emby/Sessions/missing/Message?Text=Hello&Header=Notice"),
                Some(&self.user_token),
                Body::empty(),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
        );
    }

    async fn assert_generated_query_request(&self) {
        let path = format!(
            "/emby/sEsSiOnS/{}/mEsSaGe?Text=first&tExT=Hello+mobile&Header=first&hEaDeR=Notice&TimeoutMs=1&tImEoUtMs=1500",
            self.target_session_id
        );
        let response = request(
            &self.emby,
            Method::POST,
            &path,
            Some(&self.user_token),
            Body::empty(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_bytes(response).await.is_empty());

        let queued = self.queued_commands().await;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].message_type, "GeneralCommand");
        assert_eq!(queued[0].payload["Name"], "DisplayMessage");
        assert_eq!(queued[0].payload["Arguments"]["Text"], "Hello mobile");
        assert_eq!(queued[0].payload["Arguments"]["Header"], "Notice");
        assert_eq!(queued[0].payload["Arguments"]["TimeoutMs"], "1500");
    }

    async fn assert_api_key_request(&self) {
        let path = format!(
            "/emby/Sessions/{}/Message?Text=API+key&Header=Notice",
            self.target_session_id
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &path,
                Some(&self.api_key),
                Body::empty(),
            )
            .await
            .status(),
            StatusCode::OK,
        );
        assert_eq!(self.queued_commands().await.len(), 2);
    }

    async fn assert_jellyfin_body_contract(&self) {
        for prefix in ["", "/api"] {
            let path = format!(
                "{prefix}/Sessions/{}/Message?Text=query&Header=query",
                self.target_session_id
            );
            assert_eq!(
                request(
                    &self.jellyfin,
                    Method::POST,
                    &path,
                    Some(&self.user_token),
                    Body::empty(),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "Jellyfin query-only message must stay invalid: {path}",
            );

            let path = format!("{prefix}/Sessions/{}/Message", self.target_session_id);
            let body = Body::from(
                serde_json::to_vec(&json!({"Text": "Jellyfin body", "TimeoutMs": 10})).unwrap(),
            );
            assert_eq!(
                json_request(
                    &self.jellyfin,
                    Method::POST,
                    &path,
                    Some(&self.user_token),
                    body,
                )
                .await
                .status(),
                StatusCode::NO_CONTENT,
                "Jellyfin JSON body/status must stay unchanged: {path}",
            );
        }
        let queued = self.queued_commands().await;
        assert_eq!(queued.len(), 4);
        for command in &queued[2..] {
            assert_eq!(command.payload["Arguments"]["Text"], "Jellyfin body");
            assert_eq!(
                command.payload["Arguments"]["Header"],
                "Message from Server"
            );
        }
    }

    async fn queued_commands(&self) -> Vec<jellyfin_data::entities::session_command::Model> {
        SessionCommandRepository::new(self.database.clone())
            .list_for_session(&self.target_session_id)
            .await
            .expect("queued commands")
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> axum::response::Response {
    request_with_content_type(app, method, uri, token, body, false).await
}

async fn json_request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> axum::response::Response {
    request_with_content_type(app, method, uri, token, body, true).await
}

async fn request_with_content_type(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Body,
    json: bool,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if json {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("route response")
}

async fn response_bytes(response: axum::response::Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), 1024)
        .await
        .expect("bounded response")
}

fn jellyfin_session_id(app_name: &str, device_id: &str) -> String {
    let mut hasher = Md5::new();
    for unit in format!("{app_name}{device_id}").encode_utf16() {
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

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
