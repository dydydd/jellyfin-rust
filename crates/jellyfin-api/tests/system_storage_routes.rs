use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{UserService, VirtualFolderService};
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"System Storage Tests\", DeviceId=\"system-storage-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_system_storage_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn system_storage_matches_official_elevated_contract_and_lists_library_paths() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.get("/System/Info/Storage", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get("/System/Info/Storage", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let storage = body_json(
        fixture
            .get("/System/Info/Storage", Some(&fixture.admin_token))
            .await,
    )
    .await;
    assert_folder(&storage["ProgramDataFolder"], fixture.path("programdata"));
    assert_folder(&storage["WebFolder"], fixture.path("web"));
    assert_folder(&storage["ImageCacheFolder"], fixture.path("cache/images"));
    assert_folder(&storage["CacheFolder"], fixture.path("cache"));
    assert_folder(&storage["LogFolder"], fixture.path("logs"));
    assert_folder(&storage["InternalMetadataFolder"], fixture.path("metadata"));
    assert_folder(
        &storage["TranscodingTempFolder"],
        fixture.path("transcodes"),
    );
    assert!(storage.get("program_data_folder").is_none());

    let libraries = storage["Libraries"].as_array().expect("libraries");
    assert_eq!(libraries.len(), 2);
    assert_library(libraries, "Movies", fixture.path("movies"));
    assert_library(libraries, "Shows", fixture.path("shows"));

    let api_key_storage = body_json(
        fixture
            .get(
                &format!("/System/Info/Storage?api_key={}", fixture.api_key_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(
        api_key_storage["Libraries"]
            .as_array()
            .expect("libraries")
            .len(),
        2
    );

    fixture.cleanup().await;
}

fn assert_folder(folder: &Value, expected_path: impl AsRef<Path>) {
    assert_eq!(folder["Path"], path_string(expected_path.as_ref()));
    assert!(folder["FreeSpace"].as_i64().expect("free space") >= 0);
    assert!(folder["UsedSpace"].as_i64().expect("used space") >= 0);
    assert!(folder.get("ResolvedPath").is_none());
}

fn assert_library(libraries: &[Value], name: &str, expected_path: impl AsRef<Path>) {
    let library = libraries
        .iter()
        .find(|library| library["Name"] == name)
        .expect("library");
    assert!(library["Id"].as_str().expect("library id").len() == 32);
    let folders = library["Folders"].as_array().expect("folders");
    assert_eq!(folders.len(), 1);
    assert_folder(&folders[0], expected_path);
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

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    temporary: TempDirectory,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert_temporary_database_name(&database_name);
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");
        administrator.close().await.unwrap();

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

        let temporary = TempDirectory::new();
        for relative in [
            "programdata",
            "web",
            "cache/images",
            "logs",
            "metadata",
            "transcodes",
            "movies",
            "shows",
        ] {
            fs::create_dir_all(temporary.path().join(relative)).unwrap();
        }

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("system-storage-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("system-storage-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("system-storage-key-{suffix}"))
            .await
            .unwrap()
            .access_token;
        let virtual_folders = VirtualFolderService::new(database.clone());
        virtual_folders
            .create(
                "Movies",
                Some("movies".to_owned()),
                json!({ "Enabled": true }),
                vec![path_string(&temporary.path().join("movies"))],
                false,
            )
            .await
            .unwrap();
        virtual_folders
            .create(
                "Shows",
                Some("tvshows".to_owned()),
                json!({ "Enabled": true }),
                vec![path_string(&temporary.path().join("shows"))],
                false,
            )
            .await
            .unwrap();

        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "System Storage Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_log_directory(temporary.path().join("logs"))
            .with_transcode_directory(temporary.path().join("transcodes"))
            .with_storage_paths(
                temporary.path().join("programdata"),
                temporary.path().join("web"),
                temporary.path().join("cache/images"),
                temporary.path().join("cache"),
                temporary.path().join("metadata"),
            ),
        );

        Self {
            database_name,
            database,
            app,
            temporary,
            admin_token,
            user_token,
            api_key_token,
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

    fn path(&self, relative: &str) -> PathBuf {
        self.temporary.path().join(relative)
    }

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "System Storage Tests",
            "1.0",
            "Test",
            format!("system-storage-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-system-storage-route-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).unwrap();
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

fn path_string(path: &Path) -> String {
    path.to_str().expect("UTF-8 path").to_owned()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
