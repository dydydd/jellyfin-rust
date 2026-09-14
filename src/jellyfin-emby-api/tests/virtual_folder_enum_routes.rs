use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{UserService, VirtualFolderService};
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Virtual Folder Enum Tests\", DeviceId=\"emby-vf-enum-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_virtual_folder_enum_";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[tokio::test]
async fn emby_virtual_folder_lists_filter_profile_image_options_only() {
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
    let administrator = users
        .create_initial_administrator("emby-virtual-folder-enum-admin")
        .await
        .expect("administrator creation");
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            administrator.id,
            "Emby Virtual Folder Enum Tests",
            "1.0",
            "Test",
            "emby-vf-enum-tests",
        ))
        .await
        .expect("administrator session")
        .access_token;

    let virtual_folders = VirtualFolderService::new(database.clone());
    virtual_folders
        .create(
            "Enum compatibility library",
            Some("movies".to_owned()),
            json!({
                "TypeOptions": [{
                    "Type": "Movie",
                    "ImageOptions": [
                        {"Type": "Primary", "Limit": 1, "MinWidth": 600},
                        {"Type": "Profile", "Limit": 1, "MinWidth": 256},
                        {"Type": "Backdrop", "Limit": 2, "MinWidth": 1280}
                    ],
                    "FutureTypeOption": "preserved"
                }],
                "FutureLibraryOption": {"Preserved": true}
            }),
            Vec::new(),
            false,
        )
        .await
        .expect("virtual folder creation");

    let state = AppState::new(
        database.clone(),
        "Emby Virtual Folder Enum Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    );
    let app = jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state));

    for path in ["/Library/VirtualFolders", "/api/Library/VirtualFolders"] {
        let response = get_json(&app, path, &token).await;
        let folder = first_folder(&response);
        assert_eq!(image_types(folder), vec!["Primary", "Profile", "Backdrop"]);
        assert_eq!(
            folder["LibraryOptions"]["FutureLibraryOption"]["Preserved"], true,
            "Jellyfin extension fields must remain intact at {path}"
        );
    }

    for path in [
        "/emby/Library/VirtualFolders",
        "/emby/Library/VirtualFolders/Query",
        "/emby/lIbRaRy/vIrTuAlFoLdErS/qUeRy",
    ] {
        let response = get_json(&app, path, &token).await;
        let folder = first_folder(&response);
        assert_eq!(image_types(folder), vec!["Primary", "Backdrop"], "{path}");
        assert_eq!(
            folder["LibraryOptions"]["TypeOptions"][0]["FutureTypeOption"], "preserved",
            "the protocol adapter must retain unrelated fields at {path}"
        );
        assert_eq!(
            folder["LibraryOptions"]["FutureLibraryOption"]["Preserved"], true,
            "the protocol adapter must retain library extensions at {path}"
        );
    }

    drop(app);
    drop(virtual_folders);
    database.close().await.expect("database cleanup");
}

async fn get_json(app: &Router, path: &str, token: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response");
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
            .await
            .expect("bounded response body"),
    )
    .unwrap_or_else(|error| panic!("{path} returned invalid JSON: {error}"))
}

fn first_folder(response: &Value) -> &Value {
    response
        .as_array()
        .or_else(|| response["Items"].as_array())
        .and_then(|folders| folders.first())
        .expect("virtual folder response item")
}

fn image_types(folder: &Value) -> Vec<&str> {
    folder["LibraryOptions"]["TypeOptions"][0]["ImageOptions"]
        .as_array()
        .expect("image options")
        .iter()
        .map(|option| option["Type"].as_str().expect("image option type"))
        .collect()
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
