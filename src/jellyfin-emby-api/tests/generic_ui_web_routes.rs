use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use jellyfin_model::{PluginInfo, PluginStatus};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby UI Tests\", DeviceId=\"emby-ui-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_ui_web_";

#[tokio::test]
async fn generic_ui_and_web_strings_match_official_protocol_contracts() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
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
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_public_web_strings_contract().await;
    fixture.assert_generic_ui_administrator_contract().await;
    fixture.assert_protocol_isolation().await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    plugin_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-ui-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-ui-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-ui-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let plugin_id = Uuid::new_v4();
        let state = AppState::new(
            database,
            "Emby UI Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_plugins(vec![PluginInfo {
            name: "Metadata-only test plugin".to_owned(),
            version: "1.0.0.0".to_owned(),
            configuration_file_name: None,
            description: "No translation provider is registered".to_owned(),
            id: plugin_id,
            can_uninstall: true,
            has_image: false,
            status: PluginStatus::Active,
        }]);
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            plugin_id,
            admin_token,
            user_token,
            api_key,
        }
    }

    async fn assert_public_web_strings_contract(&self) {
        let id = self.plugin_id.hyphenated();
        for (path, expected) in [
            (format!("/emby/web/stringset?PluginId={id}"), json!([])),
            (format!("/emby/WeB/StRiNgSeT?pLuGiNiD={id}"), json!([])),
        ] {
            let response = request(&self.emby, Method::GET, &path, None, None).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/json; charset=utf-8",
                "{path}",
            );
            assert_eq!(response_json(response).await, expected, "{path}");
        }

        for path in [
            format!("/emby/web/strings?PluginId={id}&Locale=en-US"),
            format!("/emby/WeB/StRiNgS?pLuGiNiD={id}&lOcAlE=zh-CN"),
        ] {
            let response = request(&self.emby, Method::GET, &path, None, None).await;
            assert_plain(
                response,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Object reference not set to an instance of an object.",
            )
            .await;
        }

        let last_duplicate_wins = format!("/emby/web/stringset?PluginId=not-a-guid&PLUGINID={id}");
        assert_eq!(
            request(&self.emby, Method::GET, &last_duplicate_wins, None, None,)
                .await
                .status(),
            StatusCode::OK,
        );

        for (path, expected_body) in [
            (
                "/emby/web/stringset?PluginId=not-a-guid".to_owned(),
                "Unrecognized Guid format.",
            ),
            (
                format!("/emby/web/strings?PluginId={}&Locale=en-US", Uuid::new_v4()),
                "Sequence contains no matching element",
            ),
            (
                "/emby/web/strings".to_owned(),
                "Sequence contains no matching element",
            ),
        ] {
            let response = request(&self.emby, Method::GET, &path, None, None).await;
            assert_eq!(
                response.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{path}",
            );
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "text/plain",
                "{path}"
            );
            assert_eq!(response_text(response).await, expected_body, "{path}");
        }
    }

    async fn assert_generic_ui_administrator_contract(&self) {
        let malformed_get = "/emby/UI/View?ClientLocale=en-US";
        assert_eq!(
            request(&self.emby, Method::GET, malformed_get, None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            request(
                &self.emby,
                Method::GET,
                malformed_get,
                Some(&self.user_token),
                None,
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
        );

        for token in [&self.admin_token, &self.api_key] {
            let missing_page =
                request(&self.emby, Method::GET, malformed_get, Some(token), None).await;
            assert_plain(
                missing_page,
                StatusCode::BAD_REQUEST,
                "Value cannot be null. (Parameter 'key')",
            )
            .await;

            for path in [
                "/emby/UI/View?PageId=missing&ClientLocale=en-US",
                "/emby/uI/vIeW?pAgEiD=missing&cLiEnTlOcAlE=zh-CN",
                // ApiMember marks ClientLocale required for generated clients,
                // but Emby's runtime binder allows it to be omitted.
                "/emby/UI/View?PageId=missing",
            ] {
                let response = request(&self.emby, Method::GET, path, Some(token), None).await;
                assert_plain(
                    response,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Unable to find the specified target page (ID: missing)",
                )
                .await;
            }
        }

        let invalid_json = Some(("application/json", "{"));
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/UI/Command",
                None,
                invalid_json
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/UI/Command",
                Some(&self.user_token),
                invalid_json,
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
        );

        let empty = request(
            &self.emby,
            Method::POST,
            "/emby/UI/Command",
            Some(&self.admin_token),
            Some(("application/json", "{}")),
        )
        .await;
        assert_plain(
            empty,
            StatusCode::BAD_REQUEST,
            "Value cannot be null. (Parameter 'key')",
        )
        .await;

        let command = json!({
            "PageId": "first",
            "CommandId": "save",
            "Data": "opaque",
            "ItemId": "item",
            "ClientLocale": "de-DE",
            "pAgEiD": "missing",
            "cLiEnTlOcAlE": "en-US",
            "Unknown": true
        })
        .to_string();
        let response = request(
            &self.emby,
            Method::POST,
            "/emby/uI/cOmMaNd",
            Some(&self.admin_token),
            Some(("application/json", &command)),
        )
        .await;
        assert_plain(
            response,
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unable to find the specified target page (ID: missing)",
        )
        .await;
    }

    async fn assert_protocol_isolation(&self) {
        for (method, path, body) in [
            (Method::GET, "/UI/View?PageId=x&ClientLocale=en-US", None),
            (
                Method::GET,
                "/api/UI/View?PageId=x&ClientLocale=en-US",
                None,
            ),
            (
                Method::POST,
                "/UI/Command",
                Some(("application/json", "{}")),
            ),
            (
                Method::POST,
                "/api/UI/Command",
                Some(("application/json", "{}")),
            ),
            (Method::GET, "/web/strings", None),
            (Method::GET, "/api/web/strings", None),
            (Method::GET, "/web/stringset", None),
            (Method::GET, "/api/web/stringset", None),
        ] {
            assert_eq!(
                request(&self.jellyfin, method, path, Some(&self.admin_token), body,)
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "Emby-owned route leaked at {path}",
            );
        }
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<(&str, &str)>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    let body = if let Some((content_type, body)) = body {
        builder = builder.header(header::CONTENT_TYPE, content_type);
        Body::from(body.to_owned())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).expect("UI test request"))
        .await
        .expect("UI test response")
}

async fn assert_plain(response: Response, status: StatusCode, expected_body: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/plain");
    assert_eq!(response_text(response).await, expected_body);
}

async fn response_text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded response body")
            .to_vec(),
    )
    .expect("UTF-8 response")
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded JSON response"),
    )
    .expect("JSON response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby UI Tests",
            "1.0",
            "Test",
            format!("emby-ui-tests-{suffix}"),
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
