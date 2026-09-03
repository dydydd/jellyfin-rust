#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{UserService, VirtualFolderService};
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use std::path::Path;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Library Media Folders Tests\", DeviceId=\"library-media-folders-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_library_media_folders_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn library_media_folders_match_official_admin_contract() {
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
        exercise_library_media_folders(&task_database_name).await;
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

async fn exercise_library_media_folders(database_name: &str) {
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
    let temp_root = std::env::temp_dir().join(format!("jellyfin-library-media-folders-{suffix}"));
    let movies_path = temp_root.join("movies");
    let hidden_path = temp_root.join("hidden");
    std::fs::create_dir_all(&movies_path).expect("movies temp directory");
    std::fs::create_dir_all(&hidden_path).expect("hidden temp directory");
    let movies_path = canonical_path(&movies_path);
    let hidden_path = canonical_path(&hidden_path);

    let users = UserService::new(database.clone());
    let admin = users
        .create_initial_administrator(&format!("media-folders-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("media-folders-user-{suffix}"))
        .await
        .expect("user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
    let api_key_token = ApiKeyRepository::new(database.clone())
        .create(&format!("media-folders-key-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;

    let virtual_folders = VirtualFolderService::new(database.clone());
    virtual_folders
        .create(
            &format!("Movies {suffix}"),
            Some("movies".to_owned()),
            json!({ "Enabled": true }),
            vec![movies_path.clone()],
            false,
        )
        .await
        .expect("movies folder");
    virtual_folders
        .create(
            &format!("Hidden {suffix}"),
            Some("tvshows".to_owned()),
            json!({ "Enabled": true, "IsHidden": true }),
            vec![hidden_path.clone()],
            false,
        )
        .await
        .expect("hidden folder");
    virtual_folders
        .create(
            &format!("Disabled {suffix}"),
            Some("music".to_owned()),
            json!({ "Enabled": false }),
            Vec::new(),
            false,
        )
        .await
        .expect("disabled folder");
    virtual_folders
        .create(
            &format!("DefaultEnabled {suffix}"),
            None,
            json!({}),
            Vec::new(),
            false,
        )
        .await
        .expect("default enabled folder");

    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Library Media Folders Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    assert_eq!(
        get(&app, "/Library/MediaFolders", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, "/Library/PhysicalPaths", Some(&user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&app, "/Library/MediaFolders", Some(&user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &app,
            "/Library/MediaFolders?isHidden=maybe",
            Some(&admin_token)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let all = get_json(&app, "/Library/MediaFolders", &admin_token).await;
    assert_eq!(all["StartIndex"], 0);
    assert_eq!(all["TotalRecordCount"], 3);
    assert_eq!(
        names(&all),
        vec![
            format!("DefaultEnabled {suffix}"),
            format!("Hidden {suffix}"),
            format!("Movies {suffix}"),
        ]
    );
    assert!(
        all["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "CollectionFolder" && item["IsFolder"] == true)
    );
    assert!(
        all["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Name"] != format!("Disabled {suffix}"))
    );

    let visible = get_json(&app, "/Library/MediaFolders?isHidden=false", &admin_token).await;
    assert_eq!(
        names(&visible),
        vec![
            format!("DefaultEnabled {suffix}"),
            format!("Movies {suffix}"),
        ]
    );
    let hidden = get_json(&app, "/Library/MediaFolders?IsHidden=true", &admin_token).await;
    assert_eq!(names(&hidden), vec![format!("Hidden {suffix}")]);
    let api_key = get_json(
        &app,
        &format!("/Library/MediaFolders?api_key={api_key_token}"),
        "",
    )
    .await;
    assert_eq!(api_key["TotalRecordCount"], 3);

    let physical_paths = get_json(&app, "/Library/PhysicalPaths", &admin_token).await;
    assert_eq!(
        string_array(&physical_paths),
        vec![hidden_path.clone(), movies_path.clone()]
    );
    let api_key_paths = get_json(
        &app,
        &format!("/Library/PhysicalPaths?api_key={api_key_token}"),
        "",
    )
    .await;
    assert_eq!(string_array(&api_key_paths), vec![hidden_path, movies_path]);

    std::fs::remove_dir_all(&temp_root).expect("temporary media path cleanup");
    database.close().await.expect("database pool cleanup");
}

async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
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

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let token = (!token.is_empty()).then_some(token);
    let response = get(app, uri, token).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn names(response: &Value) -> Vec<String> {
    response["Items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["Name"].as_str().expect("name").to_owned())
        .collect()
}

fn string_array(response: &Value) -> Vec<String> {
    response
        .as_array()
        .expect("string array")
        .iter()
        .map(|value| value.as_str().expect("string").to_owned())
        .collect()
}

fn canonical_path(path: &Path) -> String {
    path.canonicalize()
        .expect("canonical path")
        .to_str()
        .expect("UTF-8 path")
        .to_owned()
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Library Media Folders Tests",
            "1.0",
            "Test",
            format!("library-media-folders-tests-{suffix}"),
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
