#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository,
};
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Library Available Options Tests\", DeviceId=\"library-available-options-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_library_available_options_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn library_available_options_match_official_startup_policy_and_type_defaults() {
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
        exercise_library_available_options(&task_database_name).await;
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

async fn exercise_library_available_options(database_name: &str) {
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
        .create_initial_administrator(&format!("available-options-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("available-options-user-{suffix}"))
        .await
        .expect("user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
    let api_key_token = ApiKeyRepository::new(database.clone())
        .create(&format!("available-options-key-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;
    let server_configuration = ServerConfigurationRepository::new(database.clone());

    let app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "Library Available Options Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_persistent_startup(server_configuration.clone()),
    );

    let first_time = get_json(&app, "/Libraries/AvailableOptions", None).await;
    assert_eq!(
        type_names(&first_time),
        ["Series", "Season", "Episode", "Movie"]
    );
    assert_empty_provider_lists(&first_time);
    assert_image_option(
        &type_option(&first_time, "Movie")["DefaultImageOptions"][0],
        "Backdrop",
        1,
        1280,
    );

    assert_eq!(
        get(
            &app,
            "/Libraries/AvailableOptions?libraryContentType=definitely-not-real",
            None
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    server_configuration
        .complete_startup()
        .await
        .expect("startup completion");

    assert_eq!(
        get(&app, "/Libraries/AvailableOptions", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, "/Libraries/AvailableOptions", Some(&user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let tv = get_json(
        &app,
        "/Libraries/AvailableOptions?libraryContentType=tvshows&isNewLibrary=true",
        Some(&admin_token),
    )
    .await;
    assert_eq!(type_names(&tv), ["Series", "Season", "Episode"]);
    assert_image_option(
        &type_option(&tv, "Season")["DefaultImageOptions"][1],
        "Primary",
        1,
        0,
    );

    let movies = get_json(
        &app,
        &format!("/Libraries/AvailableOptions?ApiKey={api_key_token}&libraryContentType=movies"),
        None,
    )
    .await;
    assert_eq!(type_names(&movies), ["Movie"]);
    assert_image_option(
        &type_option(&movies, "Movie")["DefaultImageOptions"][6],
        "Logo",
        1,
        0,
    );

    let books = get_json(
        &app,
        "/Libraries/AvailableOptions?LibraryContentType=books",
        Some(&admin_token),
    )
    .await;
    assert_eq!(type_names(&books), ["Book", "AudioBook"]);
    assert!(
        type_option(&books, "Book")["DefaultImageOptions"]
            .as_array()
            .expect("default image options")
            .is_empty()
    );

    database.close().await.expect("database pool cleanup");
}

async fn get(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
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

async fn get_json(app: &Router, uri: &str, token: Option<&str>) -> Value {
    let response = get(app, uri, token).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn type_names(response: &Value) -> Vec<&str> {
    response["TypeOptions"]
        .as_array()
        .expect("type options")
        .iter()
        .map(|option| option["Type"].as_str().expect("type"))
        .collect()
}

fn type_option<'a>(response: &'a Value, item_type: &str) -> &'a Value {
    response["TypeOptions"]
        .as_array()
        .expect("type options")
        .iter()
        .find(|option| option["Type"] == item_type)
        .expect("type option")
}

fn assert_empty_provider_lists(response: &Value) {
    for key in ["SubtitleFetchers", "LyricFetchers", "MediaSegmentProviders"] {
        assert!(
            response[key].as_array().expect(key).is_empty(),
            "{key} should be empty until provider manager integration is available"
        );
    }
}

fn assert_image_option(option: &Value, image_type: &str, limit: i64, min_width: i64) {
    assert_eq!(option["Type"], image_type);
    assert_eq!(option["Limit"], limit);
    assert_eq!(option["MinWidth"], min_width);
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Library Available Options Tests",
            "1.0",
            "Test",
            format!("library-available-options-tests-{suffix}"),
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
