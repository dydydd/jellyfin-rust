use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    USER_ROOT_FOLDER_ID,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby User Views Tests\", DeviceId=\"emby-user-views-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_user_views_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn legacy_emby_user_views_keep_the_generated_required_query_contract() {
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
        exercise_user_views_routes(&task_database_name).await;
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

async fn exercise_user_views_routes(database_name: &str) {
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
    assert_required_case_insensitive_query(&fixture).await;
    assert_authorization_and_lookup_precede_query_validation(&fixture).await;
    assert_jellyfin_routes_keep_optional_query_behavior(&fixture).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    missing_user_id: Uuid,
    channel_id: Uuid,
    admin_token: String,
    user_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-user-views-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-user-views-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("emby-user-views-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        items.ensure_user_root().await.expect("user root");
        let channel_id = Uuid::new_v4();
        let mut channel = NewBaseItem::new(channel_id, "Channel");
        channel.parent_id = Some(USER_ROOT_FOLDER_ID);
        channel.name = Some("Emby external channel".to_owned());
        channel.sort_name = channel.name.clone();
        channel.is_folder = true;
        items.create(channel).await.expect("external channel seed");

        let state = AppState::new(
            database,
            "Emby User Views Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            other_user_id: other_user.id,
            missing_user_id: Uuid::new_v4(),
            channel_id,
            admin_token,
            user_token,
        }
    }
}

async fn assert_required_case_insensitive_query(fixture: &Fixture) {
    let mixed_case = format!(
        "/emby/uSeRs/{}/vIeWs?iNcLuDeExTeRnAlCoNtEnT=false",
        fixture.user_id
    );
    let response = get_json(&fixture.emby, &mixed_case, Some(&fixture.user_token)).await;
    assert!(!contains_channel(&response, fixture.channel_id));

    let duplicate = format!(
        "/emby/Users/{}/Views?IncludeExternalContent=true&iNcLuDeExTeRnAlCoNtEnT=false",
        fixture.user_id
    );
    let response = get_json(&fixture.emby, &duplicate, Some(&fixture.user_token)).await;
    assert!(
        !contains_channel(&response, fixture.channel_id),
        "the last case-insensitive duplicate must control external views"
    );

    for query in ["", "?IncludeExternalContent=not-a-bool"] {
        let uri = format!("/emby/Users/{}/Views{query}", fixture.user_id);
        assert_eq!(
            request(&fixture.emby, &uri, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
}

async fn assert_authorization_and_lookup_precede_query_validation(fixture: &Fixture) {
    let self_invalid = format!(
        "/emby/Users/{}/Views?IncludeExternalContent=invalid",
        fixture.user_id
    );
    assert_eq!(
        request(&fixture.emby, &self_invalid, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let forbidden = format!(
        "/emby/Users/{}/Views?IncludeExternalContent=invalid",
        fixture.other_user_id
    );
    assert_eq!(
        request(&fixture.emby, &forbidden, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let missing = format!(
        "/emby/Users/{}/Views?IncludeExternalContent=invalid",
        fixture.missing_user_id
    );
    assert_eq!(
        request(&fixture.emby, &missing, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_jellyfin_routes_keep_optional_query_behavior(fixture: &Fixture) {
    for uri in [
        format!("/Users/{}/Views", fixture.user_id),
        format!("/api/Users/{}/Views", fixture.user_id),
        format!(
            "/Users/{}/Views?iNcLuDeExTeRnAlCoNtEnT=false",
            fixture.user_id
        ),
    ] {
        let response = get_json(&fixture.jellyfin, &uri, Some(&fixture.user_token)).await;
        assert!(
            contains_channel(&response, fixture.channel_id),
            "{uri} must retain Jellyfin's optional/default-true contract"
        );
    }
}

fn contains_channel(response: &Value, channel_id: Uuid) -> bool {
    response["Items"]
        .as_array()
        .expect("user view items")
        .iter()
        .any(|item| item["Id"] == channel_id.simple().to_string())
}

async fn get_json(app: &axum::Router, uri: &str, token: Option<&str>) -> Value {
    let response = request(app, uri, token).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .expect("response body");
    assert_eq!(
        status,
        StatusCode::OK,
        "{uri}: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("JSON response")
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    let authorization = token.map_or_else(
        || AUTHORIZATION.to_owned(),
        |token| format!("{AUTHORIZATION}, Token=\"{token}\""),
    );
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, authorization)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby User Views Tests",
            "1.0",
            "Test",
            format!("emby-user-views-tests-{suffix}"),
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
