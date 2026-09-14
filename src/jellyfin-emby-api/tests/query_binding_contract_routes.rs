use std::fmt::Write as _;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, SessionCommandRepository,
};
use md5::{Digest, Md5};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Query Binding Tests\", DeviceId=\"emby-query-binding-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_query_binding_";

#[tokio::test]
async fn generated_emby_query_names_are_case_insensitive_and_last_wins() {
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

    fixture.assert_viewing_binding_and_authorization().await;
    fixture
        .assert_active_encoding_binding_and_authorization()
        .await;
    fixture.assert_jellyfin_isolation().await;

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
            .create(&format!("emby-query-binding-user-{suffix}"))
            .await
            .expect("ordinary user");
        let devices = DeviceRepository::new(database.clone());
        let controller = devices
            .create_session(NewDevice::new(
                user.id,
                "Emby Query Binding Controller",
                "1.0",
                "Controller",
                format!("controller-{suffix}"),
            ))
            .await
            .expect("controller session");
        let target = devices
            .create_session(NewDevice::new(
                user.id,
                "Emby Query Binding Target",
                "1.0",
                "Target",
                format!("target-{suffix}"),
            ))
            .await
            .expect("target session");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-query-binding-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let target_session_id = jellyfin_session_id(&target.app_name, &target.device_id);
        let state = AppState::new(
            database.clone(),
            "Emby Query Binding Test Server".to_owned(),
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

    async fn assert_viewing_binding_and_authorization(&self) {
        let base = format!("/emby/Sessions/{}/Viewing", self.target_session_id);
        assert_eq!(
            request(&self.emby, Method::POST, &base, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede required-query binding",
        );
        for query in [
            "ItemType=Movie&ItemId=item-id",
            "ItemType=Movie&ItemName=Movie",
            "ItemId=item-id&ItemName=Movie",
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    &format!("{base}?{query}"),
                    Some(&self.user_token),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "required generated query must reject {query}",
            );
        }

        let path = format!(
            "/emby/sEsSiOnS/{}/vIeWiNg?ITEMTYPE=discarded&iTeMtYpE=Movie&ItemId=discarded&iTeMiD=selected-id&itemName=discarded&ITEMNAME=Selected+Movie&Unknown=ignored",
            self.target_session_id
        );
        assert_eq!(
            request(&self.emby, Method::POST, &path, Some(&self.user_token))
                .await
                .status(),
            StatusCode::OK,
        );
        let api_key_path = format!(
            "/emby/Sessions/{}/Viewing?itemtype=Series&itemid=api-key-id&itemname=API+Key",
            self.target_session_id
        );
        assert_eq!(
            request(&self.emby, Method::POST, &api_key_path, Some(&self.api_key),)
                .await
                .status(),
            StatusCode::OK,
            "generated API-key authentication must control a session",
        );

        let queued = SessionCommandRepository::new(self.database.clone())
            .list_for_session(&self.target_session_id)
            .await
            .expect("queued viewing commands");
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[0].payload["Name"], "DisplayContent");
        assert_eq!(queued[0].payload["Arguments"]["ItemType"], "Movie");
        assert_eq!(queued[0].payload["Arguments"]["ItemId"], "selected-id");
        assert_eq!(queued[0].payload["Arguments"]["ItemName"], "Selected Movie");
        assert_eq!(queued[1].payload["Arguments"]["ItemType"], "Series");
        assert_eq!(queued[1].payload["Arguments"]["ItemId"], "api-key-id");
    }

    async fn assert_active_encoding_binding_and_authorization(&self) {
        for (method, suffix) in [
            (Method::DELETE, "Videos/ActiveEncodings"),
            (Method::POST, "Videos/ActiveEncodings/Delete"),
        ] {
            let base = format!("/emby/{suffix}");
            assert_eq!(
                request(&self.emby, method.clone(), &base, None)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED,
                "authentication must precede active-encoding binding",
            );
            for query in [
                "DeviceId=device",
                "PlaySessionId=session",
                "DeviceId=%20&PlaySessionId=session",
                "DeviceId=device&PlaySessionId=%20",
            ] {
                assert_eq!(
                    request(
                        &self.emby,
                        method.clone(),
                        &format!("{base}?{query}"),
                        Some(&self.user_token),
                    )
                    .await
                    .status(),
                    StatusCode::BAD_REQUEST,
                    "required generated query must reject {query}",
                );
            }

            let mixed_suffix = if method == Method::DELETE {
                "vIdEoS/aCtIvEeNcOdInGs"
            } else {
                "vIdEoS/aCtIvEeNcOdInGs/dElEtE"
            };
            let path = format!(
                "/emby/{mixed_suffix}?DEVICEID=%20&deviceId=selected-device&PlaySessionId=%20&pLaYsEsSiOnId=selected-session&Unknown=ignored"
            );
            assert_eq!(
                request(&self.emby, method, &path, Some(&self.api_key))
                    .await
                    .status(),
                StatusCode::OK,
                "mixed-case last values and API-key auth must be accepted: {path}",
            );
        }
    }

    async fn assert_jellyfin_isolation(&self) {
        for prefix in ["", "/api"] {
            let viewing = format!(
                "{prefix}/Sessions/{}/Viewing?ItemType=discarded&itemtype=Movie&ItemId=discarded&itemid=selected&ItemName=discarded&itemname=selected",
                self.target_session_id
            );
            assert_eq!(
                request(
                    &self.jellyfin,
                    Method::POST,
                    &viewing,
                    Some(&self.user_token),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "Jellyfin duplicate-query behavior must remain unchanged: {viewing}",
            );

            for (method, suffix) in [
                (Method::DELETE, "Videos/ActiveEncodings"),
                (Method::POST, "Videos/ActiveEncodings/Delete"),
            ] {
                let path = format!(
                    "{prefix}/{suffix}?DeviceId=discarded&deviceid=selected&PlaySessionId=discarded&playsessionid=selected"
                );
                assert_eq!(
                    request(&self.jellyfin, method, &path, Some(&self.api_key))
                        .await
                        .status(),
                    StatusCode::BAD_REQUEST,
                    "Jellyfin duplicate-query behavior must remain unchanged: {path}",
                );
            }
        }

        let queued = SessionCommandRepository::new(self.database.clone())
            .list_for_session(&self.target_session_id)
            .await
            .expect("queued viewing commands after isolation requests");
        assert_eq!(queued.len(), 2);
    }
}

async fn request(
    app: &Router,
    method: Method,
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
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
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
