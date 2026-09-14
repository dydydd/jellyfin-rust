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
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby API Key Paging Tests\", DeviceId=\"emby-api-key-paging\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_auth_keys_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn auth_keys_paging_is_generated_emby_only_and_preserves_authorization_precedence() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(database_name.starts_with(DATABASE_PREFIX));
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
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary database migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_authorization_precedes_binding().await;
    fixture.assert_emby_paging_and_binding().await;
    fixture.assert_signed_int32_contract().await;
    fixture.assert_jellyfin_isolation().await;
    fixture.assert_emby_create_binding().await;

    database.close().await.expect("database close");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    user_token: String,
    api_key_token: String,
    key_names: Vec<String>,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-auth-keys-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-auth-keys-user-{suffix}"))
            .await
            .expect("ordinary user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session_token(
            &devices,
            administrator.id,
            &format!("administrator-{suffix}"),
        )
        .await;
        let user_token = session_token(&devices, user.id, &format!("user-{suffix}")).await;

        let api_keys = ApiKeyRepository::new(database.clone());
        let mut key_names = Vec::new();
        let mut api_key_token = None;
        for label in ["auth", "alpha", "bravo", "charlie", "delta"] {
            let key = api_keys
                .create(&format!("emby-auth-keys-{label}-{suffix}"))
                .await
                .expect("API key creation");
            key_names.push(key.name);
            if label == "auth" {
                api_key_token = Some(key.access_token);
            }
        }

        let state = AppState::new(
            database,
            "Emby API Key Paging Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            admin_token,
            user_token,
            api_key_token: api_key_token.expect("authentication API key"),
            key_names,
        }
    }

    async fn assert_authorization_precedes_binding(&self) {
        let malformed = "/emby/Auth/Keys?StartIndex=invalid&Limit=invalid";
        assert_eq!(
            request(&self.emby, malformed, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&self.emby, malformed, Some(&self.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    async fn assert_emby_create_binding(&self) {
        let malformed = "/emby/Auth/Keys?APP=%20%20";
        assert_eq!(
            request_method(&self.emby, Method::POST, malformed, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            request_method(&self.emby, Method::POST, malformed, Some(&self.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN,
        );
        for (token, name) in [
            (&self.admin_token, "emby-create-admin-selected"),
            (&self.api_key_token, "emby-create-api-key-selected"),
        ] {
            let path = format!("/emby/aUtH/kEyS?APP=discarded&aPp={name}&Unknown=ignored");
            assert_eq!(
                request_method(&self.emby, Method::POST, &path, Some(token))
                    .await
                    .status(),
                StatusCode::OK,
                "Emby create must accept {name}",
            );
        }

        let keys =
            response_json(request(&self.emby, "/emby/Auth/Keys", Some(&self.admin_token)).await)
                .await;
        assert!(find_app(&keys, "emby-create-admin-selected"));
        assert!(find_app(&keys, "emby-create-api-key-selected"));
        assert!(!find_app(&keys, "discarded"));

        for prefix in ["", "/api"] {
            let path = format!("{prefix}/Auth/Keys?app=discarded&App=selected");
            assert_eq!(
                request_method(&self.jellyfin, Method::POST, &path, Some(&self.admin_token))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST,
                "Jellyfin duplicate handling must remain unchanged: {path}",
            );
        }
    }

    async fn assert_emby_paging_and_binding(&self) {
        let page = response_json(
            request(
                &self.emby,
                "/emby/aUtH/kEyS?StartIndex=invalid&sTaRtInDeX=1&Limit=invalid&lImIt=2&Unknown=ignored",
                Some(&self.admin_token),
            )
            .await,
        )
        .await;
        assert_eq!(page["StartIndex"], 1);
        assert_eq!(page["TotalRecordCount"], self.key_names.len());
        assert_eq!(app_names(&page), self.key_names[1..3]);

        let api_key_page = response_json(
            request(
                &self.emby,
                "/emby/Auth/Keys?StartIndex=2&Limit=1",
                Some(&self.api_key_token),
            )
            .await,
        )
        .await;
        assert_eq!(api_key_page["TotalRecordCount"], self.key_names.len());
        assert_eq!(app_names(&api_key_page), self.key_names[2..3]);
    }

    async fn assert_signed_int32_contract(&self) {
        let negative_start = response_json(
            request(
                &self.emby,
                "/emby/Auth/Keys?StartIndex=-2147483648&Limit=2147483647",
                Some(&self.admin_token),
            )
            .await,
        )
        .await;
        assert_eq!(negative_start["StartIndex"], i32::MIN);
        assert_eq!(app_names(&negative_start), self.key_names);

        let large_start = response_json(
            request(
                &self.emby,
                "/emby/Auth/Keys?StartIndex=2147483647&Limit=2147483647",
                Some(&self.admin_token),
            )
            .await,
        )
        .await;
        assert_eq!(large_start["StartIndex"], i32::MAX);
        assert!(app_names(&large_start).is_empty());
        assert_eq!(large_start["TotalRecordCount"], self.key_names.len());

        let negative_limit = response_json(
            request(
                &self.emby,
                "/emby/Auth/Keys?StartIndex=0&Limit=-2147483648",
                Some(&self.admin_token),
            )
            .await,
        )
        .await;
        assert!(app_names(&negative_limit).is_empty());
        assert_eq!(negative_limit["TotalRecordCount"], self.key_names.len());

        for query in [
            "StartIndex=2147483648",
            "StartIndex=-2147483649",
            "Limit=2147483648",
            "Limit=-2147483649",
            "Limit=invalid",
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    &format!("/emby/Auth/Keys?{query}"),
                    Some(&self.admin_token),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "{query}"
            );
        }
    }

    async fn assert_jellyfin_isolation(&self) {
        for path in [
            "/Auth/Keys?StartIndex=1&Limit=1",
            "/api/Auth/Keys?StartIndex=1&Limit=1",
            "/Auth/Keys?StartIndex=invalid&Limit=invalid",
            "/api/Auth/Keys?StartIndex=invalid&Limit=invalid",
        ] {
            let page =
                response_json(request(&self.jellyfin, path, Some(&self.admin_token)).await).await;
            assert_eq!(page["StartIndex"], 0, "{path}");
            assert_eq!(page["TotalRecordCount"], self.key_names.len(), "{path}");
            assert_eq!(app_names(&page), self.key_names, "{path}");
        }
    }
}

async fn session_token(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby API Key Paging Tests",
            "1.0",
            "Test Browser",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn request(app: &Router, uri: &str, token: Option<&str>) -> Response {
    request_method(app, Method::GET, uri, token).await
}

async fn request_method(app: &Router, method: Method, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, AUTHORIZATION);
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn app_names(page: &Value) -> Vec<String> {
    page["Items"]
        .as_array()
        .expect("paged items")
        .iter()
        .map(|item| {
            item["AppName"]
                .as_str()
                .expect("API key app name")
                .to_owned()
        })
        .collect()
}

fn find_app(page: &Value, expected: &str) -> bool {
    app_names(page).iter().any(|name| name == expected)
}
