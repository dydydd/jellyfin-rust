use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DatabaseConfig, DeviceRepository, NamedConfigurationRepository, NewDevice,
    ServerConfigurationRepository, StartupConfigurationUpdate, entities::user,
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
    let administrator = users
        .create_initial_administrator(&format!("configuration-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("configuration-user-{suffix}"))
        .await
        .expect("user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
    let named_configurations = NamedConfigurationRepository::new(database.clone());
    named_configurations
        .save(
            "Branding",
            json!({
                "LoginDisclaimer": "旧免责声明",
                "CustomCss": "body { color: oldlace; }\n",
                "SplashscreenEnabled": false,
                "SplashscreenLocation": "/srv/jellyfin/private/splash.png"
            }),
        )
        .await
        .expect("seed branding configuration");

    assert_eq!(
        request(&app, "/System/Configuration", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "/System/Configuration/MetadataOptions/Default", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "/System/Configuration/MetadataOptions/Default",
            Some(&user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let metadata_options = body_json(
        request(
            &app,
            "/System/Configuration/MetadataOptions/Default",
            Some(&admin_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        metadata_options,
        json!({
            "ItemType": "",
            "DisabledMetadataSavers": [],
            "LocalMetadataReaderOrder": [],
            "DisabledMetadataFetchers": [],
            "MetadataFetcherOrder": [],
            "DisabledImageFetchers": [],
            "ImageFetcherOrder": []
        })
    );

    let mut configuration =
        body_json(request(&app, "/System/Configuration", Some(&user_token)).await).await;
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

    let branding = body_json(request(&app, "/Branding/Configuration", None).await).await;
    assert_eq!(branding["LoginDisclaimer"], "旧免责声明");
    assert_eq!(branding["CustomCss"], "body { color: oldlace; }\n");
    assert_eq!(branding["SplashscreenEnabled"], false);
    assert!(branding.get("SplashscreenLocation").is_none());
    assert_eq!(
        body_text(request(&app, "/Branding/Css", None).await).await,
        "body { color: oldlace; }\n"
    );
    assert_eq!(
        request(&app, "/System/Configuration/branding", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let named_branding =
        body_json(request(&app, "/System/Configuration/branding", Some(&user_token)).await).await;
    assert_eq!(named_branding["LoginDisclaimer"], "旧免责声明");
    assert_eq!(
        named_branding["SplashscreenLocation"],
        "/srv/jellyfin/private/splash.png"
    );
    assert_eq!(
        request(
            &app,
            "/System/Configuration/does-not-exist",
            Some(&user_token)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let named_configuration = json!({
        "TranscodingTempPath": "/tmp/jellyfin-transcodes",
        "EnableThrottling": true,
        "SegmentKeepSeconds": 360
    });
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Encoding",
            None,
            &named_configuration,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Encoding",
            Some(&user_token),
            &named_configuration,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Encoding",
            Some(&admin_token),
            &json!(["invalid"]),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Encoding",
            Some(&admin_token),
            &named_configuration,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        body_json(request(&app, "/System/Configuration/encoding", Some(&user_token)).await).await,
        named_configuration
    );
    let persisted_encoding = named_configurations
        .load("encoding")
        .await
        .expect("encoding configuration load");
    assert_eq!(persisted_encoding.configuration, named_configuration);

    let branding_update = json!({
        "LoginDisclaimer": "新的免责声明",
        "CustomCss": "body { color: #00a4dc; }\n",
        "SplashscreenEnabled": true
    });
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Branding",
            None,
            &branding_update,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Branding",
            Some(&user_token),
            &branding_update,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Branding",
            Some(&admin_token),
            &json!({ "SplashscreenEnabled": [] }),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration/Branding",
            Some(&admin_token),
            &branding_update,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    let branding = body_json(request(&app, "/Branding/Configuration", None).await).await;
    assert_eq!(branding["LoginDisclaimer"], "新的免责声明");
    assert_eq!(branding["CustomCss"], "body { color: #00a4dc; }\n");
    assert_eq!(branding["SplashscreenEnabled"], true);
    assert!(branding.get("SplashscreenLocation").is_none());
    assert_eq!(
        body_text(request(&app, "/Branding/Css.css", None).await).await,
        "body { color: #00a4dc; }\n"
    );
    let persisted_branding = named_configurations
        .load("branding")
        .await
        .expect("branding configuration load");
    assert_eq!(
        persisted_branding.configuration["SplashscreenLocation"],
        "/srv/jellyfin/private/splash.png"
    );
    assert_eq!(
        persisted_branding.configuration["LoginDisclaimer"],
        "新的免责声明"
    );

    let updated = configuration
        .as_object_mut()
        .expect("server configuration object");
    updated.insert(
        "ServerName".to_owned(),
        json!("Updated Configuration Server"),
    );
    updated.insert("UICulture".to_owned(), json!("ja-JP"));
    updated.insert("MetadataCountryCode".to_owned(), json!("JP"));
    updated.insert("PreferredMetadataLanguage".to_owned(), json!("ja"));
    updated.insert("IsStartupWizardCompleted".to_owned(), json!(false));
    updated.insert(
        "ContentTypes".to_owned(),
        json!([{ "Name": "/anime", "Value": "tvshows" }]),
    );
    updated.insert(
        "PluginRepositories".to_owned(),
        json!([
            {
                "Name": "Nightly",
                "Url": "https://repo.example.test/nightly.json",
                "Enabled": false
            }
        ]),
    );
    updated.insert("MinResumePct".to_owned(), json!(8));
    updated.insert("MaxResumePct".to_owned(), json!(88));
    updated.insert("MinResumeDurationSeconds".to_owned(), json!(480));
    updated.insert("MinAudiobookResume".to_owned(), json!(9));
    updated.insert("MaxAudiobookResume".to_owned(), json!(11));
    updated.insert("AllowClientLogUpload".to_owned(), json!(true));
    updated.insert("QuickConnectAvailable".to_owned(), json!(false));
    updated["TrickplayOptions"]["Interval"] = json!(2_500);
    updated["TrickplayOptions"]["WidthResolutions"] = json!([320, 640]);

    assert_eq!(
        post_json(&app, "/System/Configuration", None, &configuration)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration",
            Some(&user_token),
            &configuration
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_json(
            &app,
            "/System/Configuration",
            Some(&admin_token),
            &json!({ "ServerName": [] }),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let response = post_json(
        &app,
        "/System/Configuration",
        Some(&admin_token),
        &configuration,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );

    let saved = body_json(request(&app, "/System/Configuration", Some(&user_token)).await).await;
    assert_eq!(saved["ServerName"], "Updated Configuration Server");
    assert_eq!(saved["UICulture"], "ja-JP");
    assert_eq!(saved["MetadataCountryCode"], "JP");
    assert_eq!(saved["PreferredMetadataLanguage"], "ja");
    assert_eq!(saved["IsStartupWizardCompleted"], false);
    assert_eq!(saved["ContentTypes"][0]["Name"], "/anime");
    assert_eq!(saved["ContentTypes"][0]["Value"], "tvshows");
    assert_eq!(saved["PluginRepositories"][0]["Enabled"], false);
    assert_eq!(saved["MinResumePct"], 8);
    assert_eq!(saved["MaxResumePct"], 88);
    assert_eq!(saved["MinResumeDurationSeconds"], 480);
    assert_eq!(saved["MinAudiobookResume"], 9);
    assert_eq!(saved["MaxAudiobookResume"], 11);
    assert_eq!(saved["AllowClientLogUpload"], true);
    assert_eq!(saved["QuickConnectAvailable"], false);
    assert_eq!(saved["TrickplayOptions"]["Interval"], 2_500);
    assert_eq!(
        saved["TrickplayOptions"]["WidthResolutions"],
        json!([320, 640])
    );

    let persisted = repository.load().await.expect("server configuration load");
    assert_eq!(persisted.server_name, "Updated Configuration Server");
    assert_eq!(persisted.ui_culture, "ja-JP");
    assert_eq!(
        persisted.content_types,
        json!([{ "Name": "/anime", "Value": "tvshows" }])
    );
    assert_eq!(
        persisted.plugin_repositories,
        json!([
            {
                "Name": "Nightly",
                "Url": "https://repo.example.test/nightly.json",
                "Enabled": false
            }
        ])
    );
    assert_eq!(persisted.trickplay_options["Interval"], 2_500);
    assert!(!persisted.quick_connect_available);
    assert_eq!(
        body_json(request(&app, "/QuickConnect/Enabled", None).await).await,
        json!(false)
    );

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

async fn post_json(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    body: &Value,
) -> axum::response::Response {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
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

async fn body_text(response: axum::response::Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .to_vec(),
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
