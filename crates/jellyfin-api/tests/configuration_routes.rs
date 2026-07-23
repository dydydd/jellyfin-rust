use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository,
    StartupConfigurationUpdate, entities::user,
};
use sea_orm::{ConnectionTrait, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Configuration Tests\", DeviceId=\"configuration-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_configuration_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn system_configuration_reads_persisted_server_configuration_contract() {
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
        exercise_configuration_routes(&task_database_name).await;
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

async fn exercise_configuration_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let repository = ServerConfigurationRepository::new(database.clone());
    repository
        .update_startup_configuration(StartupConfigurationUpdate {
            server_name: "Configuration Test Server".to_owned(),
            ui_culture: "de-DE".to_owned(),
            metadata_country_code: "DE".to_owned(),
            preferred_metadata_language: "de".to_owned(),
        })
        .await
        .expect("startup configuration update");
    repository
        .complete_startup()
        .await
        .expect("startup completion");
    repository
        .update_content_type_override("/media/movies", Some("movies"))
        .await
        .expect("content type update");
    repository
        .update_plugin_repositories(json!([
            {
                "Name": "Jellyfin Stable",
                "Url": "https://repo.jellyfin.org/files/plugin/manifest.json",
                "Enabled": true
            }
        ]))
        .await
        .expect("plugin repositories update");
    repository
        .update_client_log_upload(false)
        .await
        .expect("client log upload update");

    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Initial Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let user = users
        .create(&format!("configuration-user-{suffix}"))
        .await
        .expect("user creation");
    let token = session(&DeviceRepository::new(database.clone()), user.id, &suffix).await;

    assert_eq!(
        request(&app, "/System/Configuration", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let configuration = body_json(request(&app, "/System/Configuration", Some(&token)).await).await;
    assert_eq!(configuration["ServerName"], "Configuration Test Server");
    assert_eq!(configuration["UICulture"], "de-DE");
    assert_eq!(configuration["MetadataCountryCode"], "DE");
    assert_eq!(configuration["PreferredMetadataLanguage"], "de");
    assert_eq!(configuration["IsStartupWizardCompleted"], true);
    assert_eq!(configuration["ContentTypes"][0]["Name"], "/media/movies");
    assert_eq!(configuration["ContentTypes"][0]["Value"], "movies");
    assert_eq!(
        configuration["PluginRepositories"][0]["Name"],
        "Jellyfin Stable"
    );
    assert_eq!(configuration["AllowClientLogUpload"], false);
    assert_eq!(configuration["LogFileRetentionDays"], 3);
    assert_eq!(configuration["MinResumePct"], 5);
    assert_eq!(configuration["MaxResumePct"], 90);
    assert_eq!(configuration["QuickConnectAvailable"], true);
    assert_eq!(
        configuration["MetadataOptions"][4]["DisabledMetadataFetchers"],
        json!(["TheAudioDB"])
    );
    assert_eq!(
        configuration["TrickplayOptions"]["WidthResolutions"],
        json!([320])
    );
    assert!(configuration.get("server_name").is_none());
    assert!(configuration.get("UiCulture").is_none());

    user::Entity::delete_many()
        .exec(&database)
        .await
        .expect("configuration route user cleanup");
    database.close().await.expect("database pool cleanup");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Configuration Tests",
            "1.0",
            "Test",
            format!("configuration-tests-{suffix}"),
        ))
        .await
        .expect("device session creation")
        .access_token
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
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
