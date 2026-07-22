use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DeviceRepository, NewDevice,
    entities::{user, virtual_folder},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Virtual Folder Tests\", DeviceId=\"vf-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn library_structure_controller_contract_and_success_paths() {
    let fixture = Fixture::new().await;
    assert_library_access(&fixture).await;
    let (name, id) = create_library(&fixture).await;
    assert_library_options_and_conflicts(&fixture, &name, &id).await;
    assert_library_deletion(&fixture, &name).await;
    fixture.cleanup().await;
}

async fn assert_library_access(fixture: &Fixture) {
    assert_eq!(
        fixture
            .send(Method::GET, "/Library/VirtualFolders", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(
                Method::GET,
                "/Library/VirtualFolders",
                Some(&fixture.user_token),
                None
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

async fn create_library(fixture: &Fixture) -> (String, String) {
    let name = format!("Cinéma 東京 {}", fixture.suffix);
    let create_uri = format!(
        "/Library/VirtualFolders?name={}&collectionType=movies&refreshLibrary=true",
        encoded(&name)
    );
    let response = fixture
        .send(
            Method::POST,
            &create_uri,
            Some(&fixture.admin_token),
            Some(json!({ "LibraryOptions": { "Enabled": false } })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let list = fixture.get_list().await;
    let library = list
        .as_array()
        .expect("virtual folder array")
        .iter()
        .find(|folder| folder["Name"] == name)
        .expect("created virtual folder");
    assert_eq!(library["CollectionType"], "movies");
    assert_eq!(library["LibraryOptions"]["Enabled"], false);
    assert_eq!(library["RefreshStatus"], "RefreshRequested");
    let id = library["ItemId"].as_str().expect("item id").to_owned();
    (name, id)
}

async fn assert_library_options_and_conflicts(fixture: &Fixture, name: &str, id: &str) {
    let update = fixture
        .send(
            Method::POST,
            "/Library/VirtualFolders/LibraryOptions",
            Some(&fixture.admin_token),
            Some(json!({
                "Id": id,
                "LibraryOptions": { "Enabled": true, "PathInfos": [] }
            })),
        )
        .await;
    assert_eq!(update.status(), StatusCode::NO_CONTENT);
    let list = fixture.get_list().await;
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|folder| folder["Name"] == name)
            .unwrap()["LibraryOptions"]["Enabled"],
        true
    );

    let missing_options = fixture
        .send(
            Method::POST,
            "/Library/VirtualFolders/LibraryOptions",
            Some(&fixture.admin_token),
            Some(json!({ "Id": Uuid::new_v4(), "LibraryOptions": {} })),
        )
        .await;
    assert_eq!(missing_options.status(), StatusCode::NOT_FOUND);

    let equivalent = name.replace('é', "e").to_uppercase().replace(' ', "---");
    let duplicate_uri = format!("/Library/VirtualFolders?name={}", encoded(&equivalent));
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &duplicate_uri,
                Some(&fixture.admin_token),
                Some(json!({ "LibraryOptions": {} })),
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );
}

async fn assert_library_deletion(fixture: &Fixture, name: &str) {
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                "/Library/VirtualFolders?name=doesntExist",
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let delete_uri = format!(
        "/Library/VirtualFolders?name={}&refreshLibrary=true",
        encoded(name)
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &delete_uri,
                Some(&fixture.admin_token),
                None
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn official_media_structure_controller_contract() {
    let fixture = Fixture::new().await;
    for (method, uri, body, expected) in [
        (
            Method::POST,
            "/Library/VirtualFolders/Name?name=+&newName=test",
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Name?name=test&newName=+",
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Name?name=doesnt+exist&newName=test",
            None,
            StatusCode::NOT_FOUND,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Paths",
            Some(json!({ "Name": "Test", "Path": "/this/path/doesnt/exist" })),
            StatusCode::NOT_FOUND,
        ),
        (
            Method::POST,
            "/Library/VirtualFolders/Paths/Update",
            Some(json!({ "Name": " ", "PathInfo": { "Path": "test" } })),
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::DELETE,
            "/Library/VirtualFolders/Paths?name=+",
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::DELETE,
            "/Library/VirtualFolders/Paths?name=none&path=%2Fthis%2Fpath%2Fdoesnt%2Fexist",
            None,
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            fixture
                .send(method, uri, Some(&fixture.admin_token), body)
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn media_path_mutations_validate_and_persist_real_directories() {
    let fixture = Fixture::new().await;
    let name = create_and_rename_folder(&fixture).await;
    assert_path_validation(&fixture, &name).await;
    assert_path_mutations(&fixture, &name).await;
    fixture.cleanup().await;
}

async fn create_and_rename_folder(fixture: &Fixture) -> String {
    let name = format!("Media {}", fixture.suffix);
    let create_uri = format!("/Library/VirtualFolders?name={}", encoded(&name));
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &create_uri,
                Some(&fixture.admin_token),
                Some(json!({ "LibraryOptions": {} })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let renamed = format!("Renamed {}", fixture.suffix);
    let rename_uri = format!(
        "/Library/VirtualFolders/Name?name={}&newName={}&refreshLibrary=true",
        encoded(&name),
        encoded(&renamed)
    );
    assert_eq!(
        fixture
            .send(Method::POST, &rename_uri, Some(&fixture.admin_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    renamed
}

async fn assert_path_validation(fixture: &Fixture, name: &str) {
    for (uri, body, expected) in [
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": " ", "Path": fixture.media_path }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": name, "Path": fixture.file_path }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": "missing", "Path": fixture.media_path }),
            StatusCode::NOT_FOUND,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": name, "PathInfo": { "Path": " " } }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": name, "PathInfo": { "Path": fixture.file_path } }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": name, "PathInfo": { "Path": "/this/path/doesnt/exist" } }),
            StatusCode::NOT_FOUND,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({ "Name": "missing", "PathInfo": { "Path": fixture.media_path } }),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            fixture
                .send(Method::POST, uri, Some(&fixture.admin_token), Some(body))
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
    assert_eq!(
        fixture
            .send(
                Method::POST,
                "/Library/VirtualFolders/Paths",
                Some(&fixture.user_token),
                Some(json!({ "Name": name, "Path": fixture.media_path })),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for (path, expected) in [
        (" ", StatusCode::BAD_REQUEST),
        (fixture.media_path.as_str(), StatusCode::NOT_FOUND),
    ] {
        let uri = format!(
            "/Library/VirtualFolders/Paths?name={}&path={}",
            encoded(name),
            encoded(path)
        );
        assert_eq!(
            fixture
                .send(Method::DELETE, &uri, Some(&fixture.admin_token), None)
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
}

async fn assert_path_mutations(fixture: &Fixture, name: &str) {
    assert_path_add_update_and_overlap(fixture, name).await;
    assert_stale_path_removal(fixture, name).await;
    assert_path_projection(fixture, name).await;
    let remove_uri = format!(
        "/Library/VirtualFolders/Paths?name={}&path={}&refreshLibrary=true",
        encoded(name),
        encoded(&fixture.media_path)
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &remove_uri,
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        folder(&fixture.get_list().await, name)["Locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

async fn assert_path_add_update_and_overlap(fixture: &Fixture, name: &str) {
    for (uri, body, expected) in [
        (
            "/Library/VirtualFolders/Paths?refreshLibrary=true",
            json!({
                "Name": name,
                "PathInfo": { "Path": fixture.media_path, "NetworkPath": "smb://before" }
            }),
            StatusCode::NO_CONTENT,
        ),
        (
            "/Library/VirtualFolders/Paths/Update",
            json!({
                "Name": name,
                "PathInfo": { "Path": fixture.media_path, "NetworkPath": "smb://after" }
            }),
            StatusCode::NO_CONTENT,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": name, "Path": fixture.child_path }),
            StatusCode::CONFLICT,
        ),
        (
            "/Library/VirtualFolders/Paths",
            json!({ "Name": name, "Path": fixture.stale_path }),
            StatusCode::NO_CONTENT,
        ),
    ] {
        assert_eq!(
            fixture
                .send(Method::POST, uri, Some(&fixture.admin_token), Some(body))
                .await
                .status(),
            expected,
            "{uri}"
        );
    }
}

async fn assert_stale_path_removal(fixture: &Fixture, name: &str) {
    std::fs::remove_dir(&fixture.stale_path).expect("remove stale media directory");
    let uri = format!(
        "/Library/VirtualFolders/Paths?name={}&path={}",
        encoded(name),
        encoded(&fixture.stale_path)
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &uri, Some(&fixture.admin_token), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

async fn assert_path_projection(fixture: &Fixture, name: &str) {
    let list = fixture.get_list().await;
    let library = folder(&list, name);
    let canonical = std::fs::canonicalize(&fixture.media_path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(library["Locations"], json!([canonical]));
    assert_eq!(
        library["LibraryOptions"]["PathInfos"][0]["NetworkPath"],
        "smb://after"
    );
}

fn folder(list: &Value, name: &str) -> Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|folder| folder["Name"] == name)
        .unwrap()
        .clone()
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    suffix: String,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    temp_root: std::path::PathBuf,
    media_path: String,
    child_path: String,
    stale_path: String,
    file_path: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("vf-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("vf-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("vf-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("vf-user-{suffix}")).await;
        let temp_root = std::env::temp_dir().join(format!("jellyfin-rust-vf-api-{suffix}"));
        let media = temp_root.join("media");
        let child = media.join("movies");
        let stale = temp_root.join("stale");
        let file = temp_root.join("not-a-directory.mkv");
        std::fs::create_dir_all(&child).expect("fixture directories");
        std::fs::create_dir(&stale).expect("stale fixture directory");
        std::fs::write(&file, b"not a directory").expect("fixture file");
        let media_path = media.to_string_lossy().into_owned();
        let child_path = child.to_string_lossy().into_owned();
        let stale_path = stale.to_string_lossy().into_owned();
        let file_path = file.to_string_lossy().into_owned();
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Virtual Folder Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            suffix,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            temp_root,
            media_path,
            child_path,
            stale_path,
            file_path,
        }
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        let body = if let Some(value) = body {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        } else {
            Body::empty()
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn get_list(&self) -> Value {
        let response = self
            .send(
                Method::GET,
                "/Library/VirtualFolders",
                Some(&self.admin_token),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn cleanup(self) {
        virtual_folder::Entity::delete_many()
            .filter(virtual_folder::Column::Name.contains(&self.suffix))
            .exec(&self.database)
            .await
            .expect("folder cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        std::fs::remove_dir_all(&self.temp_root).expect("directory cleanup");
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Virtual Folder Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}
