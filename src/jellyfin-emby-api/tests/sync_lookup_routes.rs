use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Sync Lookup Tests\", DeviceId=\"emby-sync-lookup-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_sync_lookup_";

#[tokio::test]
async fn retired_sync_lookups_are_authenticated_not_found_and_emby_only() {
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

    let missing_name = "/emby/sYnC/jObItEmS/ItemCase/aDdItIoNaLfIlEs";
    assert_eq!(
        request(&fixture.emby, Method::GET, missing_name, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "authentication must precede required Name binding",
    );
    assert_eq!(
        request(
            &fixture.emby,
            Method::GET,
            missing_name,
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );

    for (method, path) in [
        (Method::GET, "/emby/sYnC/jObS/JobCase"),
        (Method::GET, "/emby/sYnC/jObItEmS/ItemCase/fIlE"),
        (Method::HEAD, "/emby/sYnC/jObItEmS/ItemCase/fIlE"),
        (
            Method::GET,
            "/emby/sYnC/jObItEmS/ItemCase/aDdItIoNaLfIlEs?nAmE=first&NAME=second",
        ),
    ] {
        assert_eq!(
            request(&fixture.emby, method.clone(), path, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}",
        );
        for token in [&fixture.user_token, &fixture.api_key] {
            let response = request(&fixture.emby, method.clone(), path, Some(token)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
            if method == Method::HEAD {
                assert!(
                    to_bytes(response.into_body(), 1024)
                        .await
                        .expect("HEAD body")
                        .is_empty(),
                    "HEAD must not expose a response body",
                );
            }
        }
    }

    for path in [
        "/Sync/Jobs/job",
        "/api/Sync/Jobs/job",
        "/Sync/JobItems/item/File",
        "/api/Sync/JobItems/item/File",
        "/Sync/JobItems/item/AdditionalFiles?Name=file",
        "/api/Sync/JobItems/item/AdditionalFiles?Name=file",
    ] {
        assert_eq!(
            request(
                &fixture.jellyfin,
                Method::GET,
                path,
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "Emby-only Sync lookup leaked at {path}",
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
            .create_initial_administrator(&format!("sync-lookup-user-{suffix}"))
            .await
            .expect("test user");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Sync Lookup Tests",
                "1.0",
                "Test",
                format!("emby-sync-lookup-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("sync-lookup-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Sync Lookup Test Server".to_owned(),
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

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
