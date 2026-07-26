use std::{fs::File, io::Write, path::Path};

use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice, entities::user};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use zip::{ZipWriter, write::SimpleFileOptions};

const AUTHORIZATION: &str = "MediaBrowser Client=\"Backup Tests\", Device=\"Test\", DeviceId=\"backup-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_backup_routes_";

#[tokio::test]
async fn backup_routes_require_elevation_and_read_zip_manifests() {
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
        exercise_backup_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_backup_routes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture.get("/Backup", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get("/Backup", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let empty = fixture.get("/Backup", Some(&fixture.admin_token)).await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(body_json(empty).await, Value::Array(Vec::new()));

    let backup_directory = fixture.program_data.join("backups");
    tokio::fs::create_dir_all(&backup_directory)
        .await
        .expect("backup directory");
    let archive_path = backup_directory.join("jellyfin-backup-20260724090000.zip");
    create_backup_archive(
        &archive_path,
        &json!({
            "ServerVersion": "10.11.0",
            "BackupEngineVersion": "1.0",
            "DateCreated": "2026-07-24T09:00:00Z",
            "DatabaseTables": ["BaseItem"],
            "Options": {
                "Metadata": true,
                "Trickplay": false,
                "Subtitles": true,
                "Database": true
            }
        }),
    );
    tokio::fs::write(backup_directory.join("not-a-backup.zip"), b"not a zip")
        .await
        .expect("invalid archive");

    let archive_manifest_uri = "/Backup/Manifest?path=jellyfin-backup-20260724090000.zip";
    assert_eq!(
        fixture.get(archive_manifest_uri, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(archive_manifest_uri, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                "/Backup/Manifest?path=missing-backup.zip",
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                "/Backup/Manifest?path=not-a-backup.zip",
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let manifest = fixture
        .get(
            "/Backup/Manifest?path=%2Ftmp%2Fevil%2Fjellyfin-backup-20260724090000.zip",
            Some(&fixture.admin_token),
        )
        .await;
    assert_eq!(manifest.status(), StatusCode::OK);
    let manifest = body_json(manifest).await;
    assert_eq!(manifest["ServerVersion"], "10.11.0");
    assert_eq!(manifest["BackupEngineVersion"], "1.0");
    assert_eq!(manifest["DateCreated"], "2026-07-24T09:00:00.0000000Z");
    assert_eq!(manifest["Path"], archive_path.to_string_lossy().as_ref());
    assert_eq!(manifest["Options"]["Metadata"], true);
    assert_eq!(manifest["Options"]["Trickplay"], false);
    assert_eq!(manifest["Options"]["Subtitles"], true);
    assert_eq!(manifest["Options"]["Database"], true);

    let listed = fixture.get("/Backup", Some(&fixture.admin_token)).await;
    assert_eq!(listed.status(), StatusCode::OK);
    let body = body_json(listed).await;
    let backups = body.as_array().expect("backup list");
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0]["ServerVersion"], "10.11.0");
    assert_eq!(backups[0]["BackupEngineVersion"], "1.0");
    assert_eq!(backups[0]["DateCreated"], "2026-07-24T09:00:00.0000000Z");
    assert_eq!(backups[0]["Path"], archive_path.to_string_lossy().as_ref());
    assert_eq!(backups[0]["Options"]["Metadata"], true);
    assert_eq!(backups[0]["Options"]["Trickplay"], false);
    assert_eq!(backups[0]["Options"]["Subtitles"], true);
    assert_eq!(backups[0]["Options"]["Database"], true);

    assert_backup_create_and_restore(&fixture, &archive_path).await;

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    program_data: std::path::PathBuf,
    storage_root: std::path::PathBuf,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 4,
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
            .create_initial_administrator(&format!("backup-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("backup-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("backup-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("backup-user-{suffix}")).await;

        let storage_root = std::env::temp_dir().join(format!("jellyfin-backup-routes-{suffix}"));
        let program_data = storage_root.join("programdata");
        let app_state = AppState::new(
            database.clone(),
            "Backup Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            program_data.clone(),
            storage_root.join("web"),
            storage_root.join("cache").join("images"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
        );
        let app = jellyfin_api::router(app_state);
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            program_data,
            storage_root,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
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

    async fn post_json(
        &self,
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
        self.app
            .clone()
            .oneshot(
                request
                    .body(Body::from(body.to_string().into_bytes()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("users cleanup");
        let _ = tokio::fs::remove_dir_all(&self.storage_root).await;
        self.database.close().await.unwrap();
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Backup Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn assert_backup_create_and_restore(fixture: &Fixture, existing_archive_path: &Path) {
    let create_body = json!({
        "Metadata": true,
        "Trickplay": true,
        "Subtitles": false,
        "Database": true
    });
    assert_eq!(
        fixture
            .post_json("/Backup/Create", None, &create_body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post_json("/Backup/Create", Some(&fixture.user_token), &create_body)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let created = fixture
        .post_json("/Backup/Create", Some(&fixture.admin_token), &create_body)
        .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = body_json(created).await;
    assert_eq!(created["ServerVersion"], env!("CARGO_PKG_VERSION"));
    assert_eq!(created["BackupEngineVersion"], "1.0");
    assert_eq!(created["Options"]["Metadata"], true);
    assert_eq!(created["Options"]["Trickplay"], true);
    assert_eq!(created["Options"]["Subtitles"], false);
    assert_eq!(created["Options"]["Database"], true);
    let created_path = Path::new(created["Path"].as_str().expect("created backup path"));
    assert!(created_path.starts_with(fixture.program_data.join("backups")));
    assert_eq!(
        created_path.extension().and_then(|value| value.to_str()),
        Some("zip")
    );

    let created_manifest = fixture
        .get(
            &format!(
                "/Backup/Manifest?path={}",
                created_path.file_name().unwrap().to_string_lossy()
            ),
            Some(&fixture.admin_token),
        )
        .await;
    assert_eq!(created_manifest.status(), StatusCode::OK);
    let created_manifest = body_json(created_manifest).await;
    assert_eq!(created_manifest["Path"], created["Path"]);
    assert_eq!(created_manifest["Options"], created["Options"]);

    let listed = fixture.get("/Backup", Some(&fixture.admin_token)).await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = body_json(listed).await;
    let listed_backups = listed.as_array().expect("backup list");
    assert_eq!(listed_backups.len(), 2);
    assert!(
        listed_backups
            .iter()
            .any(|backup| backup["Path"] == created["Path"])
    );

    let restore_body = json!({
        "ArchiveFileName": existing_archive_path.to_string_lossy()
    });
    assert_eq!(
        fixture
            .post_json("/Backup/Restore", None, &restore_body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post_json("/Backup/Restore", Some(&fixture.user_token), &restore_body)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .post_json(
                "/Backup/Restore",
                Some(&fixture.admin_token),
                &json!({ "ArchiveFileName": "missing-backup.zip" }),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .post_json("/Backup/Restore", Some(&fixture.admin_token), &restore_body)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

fn create_backup_archive(path: &Path, manifest: &Value) {
    let file = File::create(path).expect("backup archive file");
    let mut archive = ZipWriter::new(file);
    archive
        .start_file("manifest.json", SimpleFileOptions::default())
        .expect("manifest entry");
    archive
        .write_all(
            serde_json::to_string(manifest)
                .expect("manifest JSON")
                .as_bytes(),
        )
        .expect("write manifest");
    archive.finish().expect("finish archive");
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).expect("JSON response")
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
