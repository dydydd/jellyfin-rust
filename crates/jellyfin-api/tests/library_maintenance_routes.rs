use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Library Maintenance Tests\", DeviceId=\"library-maintenance-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_library_maintenance_routes_";

#[tokio::test]
async fn library_maintenance_routes_match_official_auth_and_validation_contract() {
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
        exercise_library_maintenance_routes(&task_database_name).await;
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

async fn exercise_library_maintenance_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let admin = users
        .create_initial_administrator(&format!("maintenance-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("maintenance-user-{suffix}"))
        .await
        .expect("user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
    let api_key_token = ApiKeyRepository::new(database.clone())
        .create(&format!("maintenance-key-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;

    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Library Maintenance Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    assert_eq!(
        request(&app, "/Library/Refresh", None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "/Library/Refresh", Some(&user_token), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/Library/Refresh", Some(&admin_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            &format!("/Library/Refresh?api_key={api_key_token}"),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    for route in [
        "/Library/Series/Added?tvdbId=121361",
        "/Library/Series/Updated?TvdbId=121361",
        "/Library/Movies/Added?imdbId=tt0133093",
        "/Library/Movies/Updated?TmdbId=603",
    ] {
        assert_eq!(
            request(&app, route, None, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "{route}"
        );
        assert_eq!(
            request(&app, route, Some(&user_token), None).await.status(),
            StatusCode::NO_CONTENT,
            "{route}"
        );
    }

    assert_eq!(
        request(
            &app,
            &format!("/Library/Series/Added?api_key={api_key_token}&tvdbId=121361"),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            "/Library/Media/Updated",
            Some(&user_token),
            Some(json!({ "Updates": [{ "Path": "/media/Movies/The Matrix.mkv" }] })),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            &format!("/Library/Media/Updated?ApiKey={api_key_token}"),
            None,
            Some(json!({ "Updates": [] })),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &app,
            "/Library/Media/Updated",
            None,
            Some(json!({ "Updates": [{ "Path": "/media/Movies/The Matrix.mkv" }] })),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "/Library/Media/Updated",
            Some(&user_token),
            Some(json!({ "Updates": [{}] })),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "/Library/Media/Updated",
            Some(&user_token),
            Some(json!({ "Updates": [{ "Path": null }] })),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw_request(
            &app,
            "/Library/Media/Updated",
            Some(&user_token),
            b"{not-json".to_vec(),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    database.close().await.expect("database pool cleanup");
}

async fn request(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method(Method::POST).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    let body = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("JSON body")
    });
    if !body.is_empty() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(request.body(Body::from(body)).expect("request"))
        .await
        .expect("route response")
}

async fn raw_request(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::from(body)).expect("request"))
        .await
        .expect("route response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Library Maintenance Tests",
            "1.0",
            "Test",
            format!("library-maintenance-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(
        database_name.starts_with(DATABASE_PREFIX),
        "refusing to manage unexpected database name: {database_name}"
    );
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit()),
        "temporary database suffix must be UUID hex: {database_name}"
    );
}
