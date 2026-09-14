use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Empty Success Tests\", DeviceId=\"emby-empty-success-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_empty_success_";

#[tokio::test]
async fn generated_empty_success_is_200_only_below_emby() {
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
    let database = jellyfin_data::connect(&temporary_database_config(database_name))
        .await
        .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let users = UserService::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let administrator = users
        .create_initial_administrator(&format!("empty-success-admin-{suffix}"))
        .await
        .expect("administrator");
    let devices = DeviceRepository::new(database.clone());
    let token = devices
        .create_session(NewDevice::new(
            administrator.id,
            "Emby Empty Success Tests",
            "1.0",
            "Test",
            format!("empty-success-{suffix}"),
        ))
        .await
        .expect("administrator session")
        .access_token;

    let state = AppState::new(
        database.clone(),
        "Emby Empty Success Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    );
    let app = jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state));

    for (method, path) in [
        (Method::DELETE, "/emby/uSeRs/{id}"),
        (Method::POST, "/emby/UsErS/{id}/DeLeTe"),
    ] {
        let victim = users
            .create(&format!("emby-victim-{}-{suffix}", Uuid::new_v4().simple()))
            .await
            .expect("Emby deletion victim");
        let path = path.replace("{id}", &victim.id.simple().to_string());
        assert_empty_status(
            request(&app, method, &path, &token).await,
            StatusCode::OK,
            &path,
        )
        .await;
    }

    for (method, path) in [
        (Method::DELETE, "/Users/{id}"),
        (Method::POST, "/api/Users/{id}/Delete"),
    ] {
        let victim = users
            .create(&format!(
                "jellyfin-victim-{}-{suffix}",
                Uuid::new_v4().simple()
            ))
            .await
            .expect("Jellyfin deletion victim");
        let path = path.replace("{id}", &victim.id.simple().to_string());
        assert_empty_status(
            request(&app, method, &path, &token).await,
            StatusCode::NO_CONTENT,
            &path,
        )
        .await;
    }

    drop(app);
    database.close().await.expect("database cleanup");
}

async fn request(app: &Router, method: Method, uri: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

async fn assert_empty_status(response: axum::response::Response, expected: StatusCode, path: &str) {
    assert_eq!(response.status(), expected, "{path}");
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("bounded response body")
            .is_empty(),
        "{path}"
    );
}

fn temporary_database_config(database_name: &str) -> DatabaseConfig {
    let mut config = DatabaseConfig::default();
    let (prefix, _) = config
        .url
        .rsplit_once('/')
        .expect("database URL must include a database name");
    config.url = format!("{prefix}/{database_name}");
    config.max_connections = 8;
    config.min_connections = 1;
    config
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
