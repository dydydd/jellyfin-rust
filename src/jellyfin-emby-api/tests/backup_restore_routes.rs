use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::{AppState, SystemCommand};
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Backup Restore Tests\", DeviceId=\"emby-backup-restore\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_backup_restore_";

#[tokio::test]
async fn backup_restore_is_truthful_safe_and_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary PostgreSQL migrations");

    let storage = TempRoot::new();
    let users = UserService::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let administrator = users
        .create_initial_administrator(&format!("backup-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let ordinary = users
        .create(&format!("backup-user-{suffix}"))
        .await
        .expect("ordinary user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, ordinary.id, &format!("user-{suffix}")).await;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("backup-key-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;

    let commands = Arc::new(Mutex::new(Vec::new()));
    let command_sink = Arc::clone(&commands);
    let state = AppState::new(
        database.clone(),
        "Emby Backup Restore Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    )
    .with_storage_paths(
        storage.path().join("programdata"),
        storage.path().join("web"),
        storage.path().join("cache/images"),
        storage.path().join("cache"),
        storage.path().join("metadata"),
    )
    .with_system_commands(move |command| {
        command_sink
            .lock()
            .expect("system command lock")
            .push(command);
    });
    let jellyfin = jellyfin_api::router(state.clone());
    let emby = jellyfin_emby_api::router(state);

    assert_authorization_precedes_body_binding(&emby, &admin_token, &user_token).await;

    let no_backup = request_json(
        &emby,
        Method::POST,
        "/emby/BackupRestore/Restore",
        Some(&admin_token),
        "{}",
    )
    .await;
    assert_eq!(no_backup.status(), StatusCode::NOT_FOUND);
    assert!(commands.lock().expect("commands").is_empty());

    for (body, expected_message) in [
        (
            r#"{"RestoreServerId":true,"RESTORESERVERID":false}"#,
            "preserving the current server id",
        ),
        (
            r#"{"UseFiles":null,"usefiles":"../../library.db"}"#,
            "SQLite light-backup selection",
        ),
    ] {
        let response = request_json(
            &emby,
            Method::POST,
            "/emby/bAcKuPrEsToRe/rEsToRe",
            Some(&admin_token),
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            String::from_utf8(response_bytes(response).await)
                .expect("UTF-8 error")
                .contains(expected_message)
        );
        assert!(commands.lock().expect("commands").is_empty());
    }

    let selective = request_json(
        &emby,
        Method::POST,
        "/emby/BackupRestore/RestoreData",
        Some(&admin_token),
        r#"{"USERS":[{"SOURCEUSERID":"../../users.db","TARGETUSERID":"target"}]}"#,
    )
    .await;
    assert_eq!(selective.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(commands.lock().expect("commands").is_empty());

    for (path, token, body) in [
        (
            "/emby/bAcKuPrEsToRe/rEsToReDaTa",
            admin_token.as_str(),
            r#"{"Users":[{}],"uSeRs":[]}"#,
        ),
        (
            "/emby/BackupRestore/RestoreData",
            api_key.as_str(),
            r#"{"users":null,"Unknown":true}"#,
        ),
    ] {
        let response = request_json(&emby, Method::POST, path, Some(token), body).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(response_bytes(response).await.is_empty());
    }
    assert!(commands.lock().expect("commands").is_empty());

    let create = request_json(
        &jellyfin,
        Method::POST,
        "/Backup/Create",
        Some(&admin_token),
        "{}",
    )
    .await;
    assert_eq!(
        create.status(),
        StatusCode::OK,
        "native PostgreSQL backup creation failed: {:?}",
        response_bytes(create).await
    );

    let info = request(
        &emby,
        Method::GET,
        "/emby/bAcKuPrEsToRe/bAcKuPiNfO",
        Some(&admin_token),
        None,
    )
    .await;
    assert_eq!(info.status(), StatusCode::OK);
    let info = response_json(info).await;
    assert_eq!(info["FullBackupInfo"]["CanRestore"], true);
    assert_eq!(info["FullBackupInfo"]["IsFullBackup"], true);
    assert_eq!(info["LightBackups"], json!([]));
    let advertised_name = info["FullBackupInfo"]["Name"]
        .as_str()
        .expect("advertised native backup name");
    assert!(advertised_name.ends_with(".zip"));

    // Both top-level and nested properties retain ASP.NET's
    // case-insensitive, last-assignment-wins behavior. The final values select
    // the one operation the native PostgreSQL engine can faithfully perform.
    let restore = request_json(
        &emby,
        Method::POST,
        "/emby/BACKUPRESTORE/RESTORE",
        Some(&admin_token),
        r#"{"RestoreServerId":false,"rEsToReSeRvErId":true,"UseFiles":"old","USEFILES":null,"Unknown":"ignored"}"#,
    )
    .await;
    assert_eq!(restore.status(), StatusCode::OK);
    assert!(response_bytes(restore).await.is_empty());

    {
        let captured = commands.lock().expect("commands");
        assert_eq!(captured.len(), 1, "restore must schedule exactly once");
        let SystemCommand::Restore(path) = &captured[0] else {
            panic!("expected a restore system command, got {:?}", captured[0]);
        };
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(advertised_name)
        );
        assert_eq!(
            path.parent(),
            Some(storage.path().join("programdata/backups").as_path())
        );
    }

    for path in [
        "/BackupRestore/Restore",
        "/backuprestore/restoredata",
        "/api/BackupRestore/Restore",
        "/api/backuprestore/restoredata",
    ] {
        let response = request_json(&jellyfin, Method::POST, path, Some(&admin_token), "{}").await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "Emby restore route leaked into Jellyfin at {path}",
        );
    }
    assert_eq!(
        commands.lock().expect("commands").len(),
        1,
        "Jellyfin isolation probes must not schedule another restore"
    );

    drop(emby);
    drop(jellyfin);
    drop(commands);
    database.close().await.expect("database close");
}

async fn assert_authorization_precedes_body_binding(
    emby: &Router,
    admin_token: &str,
    user_token: &str,
) {
    for path in [
        "/emby/BackupRestore/Restore",
        "/emby/BackupRestore/RestoreData",
    ] {
        assert_eq!(
            request_json(emby, Method::POST, path, None, "{")
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "{path}",
        );
        assert_eq!(
            request_json(emby, Method::POST, path, Some(user_token), "{")
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{path}",
        );
        assert_eq!(
            request_json(emby, Method::POST, path, Some(admin_token), "{")
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{path}",
        );
    }
}

async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: &str,
) -> Response {
    request(
        app,
        method,
        uri,
        token,
        Some(("application/json", body.as_bytes().to_vec())),
    )
    .await
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<(&str, Vec<u8>)>,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    let body = if let Some((content_type, bytes)) = body {
        request = request.header(header::CONTENT_TYPE, content_type);
        Body::from(bytes)
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).expect("backup restore request"))
        .await
        .expect("backup restore response")
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&response_bytes(response).await).expect("backup restore JSON response")
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded backup restore response")
        .to_vec()
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Backup Restore Tests",
            "1.0",
            "Test",
            format!("emby-backup-restore-{suffix}"),
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

struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-emby-backup-restore-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("temporary storage root");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
