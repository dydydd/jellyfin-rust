use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Typed Settings Tests\", DeviceId=\"emby-typed-settings-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_typed_settings_";
const MAX_RESPONSE_SIZE: usize = 2 * 1024 * 1024;

#[tokio::test]
async fn typed_settings_preserve_bytes_authorization_and_protocol_isolation() {
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
        exercise_typed_settings_routes(&task_database_name).await;
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

async fn exercise_typed_settings_routes(database_name: &str) {
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

    let fixture = Fixture::new(database.clone()).await;
    assert_target_authorization_precedes_body_errors(&fixture).await;
    assert_binary_round_trip_and_key_isolation(&fixture).await;
    assert_restart_persistence_and_protocol_isolation(&fixture, database.clone()).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-typed-settings-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-typed-settings-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("emby-typed-settings-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-typed-settings-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        Self {
            app: jellyfin_emby_api::router(AppState::new(
                database,
                "Emby Typed Settings Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
            api_key_token,
        }
    }
}

async fn assert_target_authorization_precedes_body_errors(fixture: &Fixture) {
    let other_route = format!(
        "/emby/Users/{}/TypedSettings/Playback",
        fixture.other_user_id
    );
    assert_eq!(
        request(&fixture.app, "GET", &other_route, None, Body::empty())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &other_route,
            Some(&fixture.user_token),
            Body::from(vec![0_u8; 3 * 1024 * 1024]),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "target authorization must precede a rejected oversized body"
    );
    let missing_route = format!("/emby/Users/{}/TypedSettings/Playback", Uuid::new_v4());
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &missing_route,
            Some(&fixture.admin_token),
            Body::from(vec![0_u8; 3 * 1024 * 1024]),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "target lookup must precede a rejected oversized body"
    );
}

async fn assert_binary_round_trip_and_key_isolation(fixture: &Fixture) {
    let canonical = format!("/emby/Users/{}/TypedSettings/Playback", fixture.user_id);
    let mixed_case = format!("/emby/uSeRs/{}/tYpEdSeTtInGs/Playback", fixture.user_id);
    let absent = get_bytes(&fixture.app, &canonical, Some(&fixture.user_token)).await;
    assert!(absent.is_empty());

    let payload = vec![0, 0xff, b'{', b'\n', 0x80, b'}'];
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &mixed_case,
            Some(&fixture.user_token),
            Body::from(payload.clone()),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        get_bytes(&fixture.app, &canonical, Some(&fixture.user_token)).await,
        payload
    );
    assert!(
        get_bytes(
            &fixture.app,
            &format!("/emby/Users/{}/TypedSettings/playback", fixture.user_id),
            Some(&fixture.user_token),
        )
        .await
        .is_empty(),
        "dynamic setting keys must retain their casing"
    );
    assert_eq!(
        get_bytes(&fixture.app, &canonical, Some(&fixture.api_key_token)).await,
        payload,
        "valid API keys may target an existing user"
    );
}

async fn assert_restart_persistence_and_protocol_isolation(
    fixture: &Fixture,
    database: DatabaseConnection,
) {
    let route = format!("/emby/Users/{}/TypedSettings/Playback", fixture.user_id);
    let restarted = jellyfin_emby_api::router(AppState::new(
        database.clone(),
        "Emby Typed Settings Restarted Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    assert_eq!(
        get_bytes(&restarted, &route, Some(&fixture.admin_token)).await,
        vec![0, 0xff, b'{', b'\n', 0x80, b'}']
    );

    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Typed Settings Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    assert_eq!(
        request(
            &jellyfin,
            "GET",
            &format!("/Users/{}/TypedSettings/Playback", fixture.user_id),
            Some(&fixture.user_token),
            Body::empty(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "typed settings must not be registered on the unprefixed Jellyfin tree"
    );
}

async fn get_bytes(app: &axum::Router, uri: &str, token: Option<&str>) -> Vec<u8> {
    let response = request(app, "GET", uri, token, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .expect("typed setting response body")
        .to_vec()
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/octet-stream");
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(body).expect("typed setting request"))
        .await
        .expect("typed setting response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Typed Settings Tests",
            "1.0",
            "Test",
            format!("emby-typed-settings-tests-{suffix}"),
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
