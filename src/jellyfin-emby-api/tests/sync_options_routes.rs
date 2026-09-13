use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Sync Options Tests\", DeviceId=\"emby-sync-options-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_sync_options_";

#[tokio::test]
async fn sync_options_is_authenticated_strict_sdk_decodable_and_emby_only() {
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

    assert_eq!(
        request(&fixture.emby, "/emby/Sync/Options", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "authentication must precede required UserId binding",
    );
    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Sync/Options",
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );

    let expected = json!({
        "Targets": [],
        "Options": [],
        "QualityOptions": [],
        "ProfileOptions": []
    });
    for (token, path) in [
        (
            fixture.user_token.as_str(),
            "/emby/sYnC/oPtIoNs?uSeRiD=first&USERID=second&itemids=1%2C2&ITEMIDS=3&PARENTID=parent&TARGETID=target&CATEGORY=nextup",
        ),
        (
            fixture.api_key.as_str(),
            "/emby/sync/options?UserId=user&Category=1",
        ),
        (
            fixture.user_token.as_str(),
            "/emby/Sync/Options?UserId=user&Category=invalid&category=Resume",
        ),
    ] {
        let response = request(&fixture.emby, path, Some(token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response_json(response).await, expected, "{path}");
    }

    for path in [
        "/emby/Sync/Options?UserId=user&Category=Resume&category=invalid",
        "/emby/Sync/Options?UserId=user&Category=0",
        "/emby/Sync/Options?UserId=user&Category=4",
        "/emby/Sync/Options?UserId=user&Category=unknown",
    ] {
        assert_eq!(
            request(&fixture.emby, path, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "invalid final Category must fail: {path}",
        );
    }

    for path in ["/Sync/Options?UserId=user", "/api/Sync/Options?UserId=user"] {
        assert_eq!(
            request(&fixture.jellyfin, path, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "Emby-only Sync/Options leaked at {path}",
        );
    }

    database.close().await.expect("database cleanup");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let user = users
            .create_initial_administrator(&format!("sync-options-user-{suffix}"))
            .await
            .expect("test user");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Sync Options Tests",
                "1.0",
                "Test",
                format!("emby-sync-options-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("sync-options-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Sync Options Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_token,
            api_key,
        }
    }
}

async fn request(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
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

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("response body");
    serde_json::from_slice(&body).expect("JSON response")
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
