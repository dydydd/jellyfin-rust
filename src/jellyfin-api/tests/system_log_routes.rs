use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;
const AUTHORIZATION: &str = "MediaBrowser Client=\"System Log Tests\", DeviceId=\"system-log-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn elevated_log_listing_filters_metadata_and_applies_official_stable_order() {
    let temporary_directory = TempDirectory::new();
    let log_directory = temporary_directory.path().join("logs");
    fs::create_dir_all(log_directory.join("nested")).unwrap();

    let latest = log_directory.join("latest.LOG");
    let created_old = log_directory.join("created-old.log");
    let created_new = log_directory.join("created-new.TXT");
    let name_last = log_directory.join("zeta.log");
    let name_first = log_directory.join("alpha.log");
    fs::write(&latest, b"latest").unwrap();
    fs::write(&created_old, b"old").unwrap();
    thread::sleep(Duration::from_millis(20));
    fs::write(&created_new, b"newer").unwrap();
    fs::write(&name_last, b"same inode").unwrap();
    fs::hard_link(&name_last, &name_first).unwrap();

    set_modified(&latest, UNIX_EPOCH + Duration::from_secs(1_700_000_300));
    let same_modified = UNIX_EPOCH + Duration::from_secs(1_700_000_200);
    set_modified(&created_old, same_modified);
    set_modified(&created_new, same_modified);
    set_modified(&name_last, UNIX_EPOCH + Duration::from_secs(1_700_000_100));

    fs::write(log_directory.join("ignored.json"), b"ignored").unwrap();
    fs::write(log_directory.join("nested/ignored.log"), b"nested").unwrap();
    fs::create_dir(log_directory.join("directory.log")).unwrap();

    let old_created = fs::metadata(&created_old).unwrap().created().unwrap();
    let new_created = fs::metadata(&created_new).unwrap().created().unwrap();
    assert!(
        new_created > old_created,
        "test requires ordered birth times"
    );
    let first_metadata = fs::metadata(&name_first).unwrap();
    let last_metadata = fs::metadata(&name_last).unwrap();
    assert_eq!(
        first_metadata.created().unwrap(),
        last_metadata.created().unwrap()
    );
    assert_eq!(
        first_metadata.modified().unwrap(),
        last_metadata.modified().unwrap()
    );

    let fixture = Fixture::new(&log_directory).await;
    assert_eq!(
        fixture.request("/System/Logs", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request("/System/Logs", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let response = fixture
        .request("/System/Logs", Some(&fixture.admin_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let logs: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    let logs = logs.as_array().unwrap();
    assert_eq!(
        logs.iter()
            .map(|log| log["Name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "latest.LOG",
            "created-new.TXT",
            "created-old.log",
            "alpha.log",
            "zeta.log"
        ]
    );
    for (log, path) in
        logs.iter()
            .zip([&latest, &created_new, &created_old, &name_first, &name_last])
    {
        assert_log_metadata(log, path);
    }

    let api_key_route = format!("/System/Logs?api_key={}", fixture.api_key_token);
    let response = fixture.request(&api_key_route, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body_bytes(response).await).unwrap(),
        serde_json::Value::Array(logs.clone())
    );

    fs::remove_dir_all(&log_directory).unwrap();
    let response = fixture
        .request("/System/Logs", Some(&fixture.admin_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"[]");

    fixture.cleanup().await;
}

#[tokio::test]
async fn elevated_identities_stream_real_logs_as_utf8_plain_text() {
    let temporary_directory = TempDirectory::new();
    let log_directory = temporary_directory.path().join("logs");
    fs::create_dir_all(&log_directory).unwrap();
    let payload = vec![b'x'; 2 * 64 * 1024 + 137];
    fs::write(log_directory.join("Server.JSON"), &payload).unwrap();
    fs::write(log_directory.join("Épisode.LOG"), b"unicode log").unwrap();
    let fixture = Fixture::new(&log_directory).await;
    let route = log_route("server.json");

    assert_eq!(
        fixture.request(&route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(&route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let response = fixture.request(&route, Some(&fixture.admin_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    assert!(!response.headers().contains_key(header::CONTENT_DISPOSITION));
    assert_eq!(body_bytes(response).await, payload);

    assert_eq!(
        fixture
            .request(
                "/System/Logs/Log?Name=server.json",
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::OK
    );
    let api_key_route = format!("{route}&api_key={}", fixture.api_key_token);
    assert_eq!(
        fixture.request(&api_key_route, None).await.status(),
        StatusCode::OK
    );
    let response = fixture
        .request(&log_route("éPISODE.log"), Some(&fixture.admin_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"unicode log");

    fixture.cleanup().await;
}

#[tokio::test]
async fn invalid_unknown_ambiguous_and_unsafe_log_names_are_rejected() {
    let temporary_directory = TempDirectory::new();
    let log_directory = temporary_directory.path().join("logs");
    let nested_directory = log_directory.join("nested");
    let sibling_directory = temporary_directory.path().join("logs-evil");
    let outside_file = temporary_directory.path().join("outside.log");
    fs::create_dir_all(&nested_directory).unwrap();
    fs::create_dir_all(&sibling_directory).unwrap();
    fs::write(log_directory.join("known.log"), b"known").unwrap();
    fs::write(nested_directory.join("nested.log"), b"nested").unwrap();
    fs::write(sibling_directory.join("hidden.log"), b"sibling").unwrap();
    fs::write(&outside_file, b"outside").unwrap();

    let mut not_found_names = vec![
        "DOES_NOT_EXIST.txt".to_owned(),
        "../outside.log".to_owned(),
        "..\\outside.log".to_owned(),
        "nested/nested.log".to_owned(),
        "hidden.log".to_owned(),
        outside_file.to_string_lossy().into_owned(),
    ];
    add_unix_unsafe_entries(
        &log_directory,
        &nested_directory,
        &outside_file,
        &mut not_found_names,
    );

    let fixture = Fixture::new(&log_directory).await;
    for route in [
        "/System/Logs/Log",
        "/System/Logs/Log?name=",
        "/System/Logs/Log?name=%20%20%20",
    ] {
        assert_eq!(
            fixture
                .request(route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for name in not_found_names {
        assert_eq!(
            fixture
                .request(&log_route(&name), Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "unsafe log name {name:?} should not resolve"
        );
    }
    let response = fixture
        .request(&log_route("KNOWN.LOG"), Some(&fixture.admin_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"known");

    fixture.cleanup().await;
}

#[cfg(unix)]
fn add_unix_unsafe_entries(
    log_directory: &Path,
    nested_directory: &Path,
    outside_file: &Path,
    not_found_names: &mut Vec<String>,
) {
    use std::os::unix::fs::symlink;

    fs::write(log_directory.join("duplicate.log"), b"first").unwrap();
    fs::write(log_directory.join("DUPLICATE.LOG"), b"second").unwrap();
    fs::write(log_directory.join("Écho.log"), b"first unicode").unwrap();
    fs::write(log_directory.join("éCHO.LOG"), b"second unicode").unwrap();
    fs::write(log_directory.join("inside.log"), b"inside").unwrap();
    symlink(outside_file, log_directory.join("outside-link.log")).unwrap();
    symlink(
        log_directory.join("inside.log"),
        log_directory.join("inside-link.log"),
    )
    .unwrap();
    symlink(nested_directory, log_directory.join("directory-link")).unwrap();
    not_found_names.extend(
        [
            "duplicate.log",
            "écho.log",
            "outside-link.log",
            "inside-link.log",
            "directory-link",
            "directory-link/nested.log",
        ]
        .map(str::to_owned),
    );
}

#[cfg(not(unix))]
fn add_unix_unsafe_entries(
    _log_directory: &Path,
    _nested_directory: &Path,
    _outside_file: &Path,
    _not_found_names: &mut Vec<String>,
) {
}

fn log_route(name: &str) -> String {
    format!(
        "/System/Logs/Log?name={}",
        utf8_percent_encode(name, NON_ALPHANUMERIC)
    )
}

fn set_modified(path: &Path, modified: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn assert_log_metadata(log: &serde_json::Value, path: &Path) {
    let metadata = fs::metadata(path).unwrap();
    let modified = metadata.modified().unwrap();
    let created = metadata.created().unwrap_or(modified);
    assert_eq!(log["Size"], i64::try_from(metadata.len()).unwrap());
    assert_eq!(
        DateTime::parse_from_rfc3339(log["DateCreated"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc),
        DateTime::<Utc>::from(created)
    );
    assert_eq!(
        DateTime::parse_from_rfc3339(log["DateModified"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc),
        DateTime::<Utc>::from(modified)
    );
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
}

impl Fixture {
    async fn new(log_directory: &Path) -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("system-log-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("system-log-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("system-log-key-{suffix}"))
            .await
            .unwrap();
        let state = AppState::new(
            database.clone(),
            "System Log Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_log_directory(log_directory);
        Self {
            database,
            app: jellyfin_api::router(state),
            admin_id: administrator.id,
            user_id: user.id,
            admin_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
        }
    }

    async fn request(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .unwrap();
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .unwrap();
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "System Log Tests",
            "1.0",
            "Test",
            format!("system-log-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .unwrap()
        .to_vec()
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-system-log-api-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
