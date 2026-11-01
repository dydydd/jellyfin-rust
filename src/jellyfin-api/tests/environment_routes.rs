use std::{fs, path::PathBuf};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_environment_routes_";
const AUTHORIZATION: &str = "MediaBrowser Client=\"Environment Tests\", Device=\"PostgreSQL\", DeviceId=\"environment\", Version=\"1.0\"";

#[tokio::test]
async fn environment_routes_match_official_file_system_and_authorization_contract() {
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
        exercise_environment_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_environment_routes(database_name: &str) {
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
    let fixture = Fixture::new(database.clone()).await;

    assert_anonymous_file_system_contract(&fixture).await;
    complete_startup(&fixture).await;
    assert_completed_setup_authorization(&fixture).await;

    drop(fixture);
    database.close().await.expect("database pool must close");
}

async fn assert_anonymous_file_system_contract(fixture: &Fixture) {
    let path = encoded(&fixture.directory.path_string());
    let base = format!("/Environment/DirectoryContents?path={path}");
    let response = send(&fixture.app, Method::GET, &base, Credential::None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, json!([]));

    let files_uri = format!("{base}&includeFiles=true");
    let files = body_json(
        send(
            &fixture.app,
            Method::GET,
            &files_uri,
            Credential::None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(files.as_array().unwrap().len(), 2);
    assert!(
        files
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["Type"] == "File")
    );
    assert!(
        files
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["Name"] == "ä movie.mkv")
    );

    let directories_uri = format!("{base}&includeDirectories=true");
    let directories = body_json(
        send(
            &fixture.app,
            Method::GET,
            &directories_uri,
            Credential::None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(directories.as_array().unwrap().len(), 2);
    assert!(
        directories
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["Name"] == "媒体")
    );

    let all_uri = format!("{base}&includeFiles=true&includeDirectories=true");
    let first =
        body_json(send(&fixture.app, Method::GET, &all_uri, Credential::None, None).await).await;
    let paths = first
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["Path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(paths.windows(2).all(|pair| pair[0] <= pair[1]));
    fs::write(fixture.directory.path().join("new.txt"), b"new").unwrap();
    let second =
        body_json(send(&fixture.app, Method::GET, &all_uri, Credential::None, None).await).await;
    assert_eq!(
        second.as_array().unwrap().len(),
        first.as_array().unwrap().len() + 1
    );

    let unc = "/Environment/DirectoryContents?path=%5C%5Cserver&includeFiles=true&includeDirectories=true";
    assert_eq!(
        body_json(send(&fixture.app, Method::GET, unc, Credential::None, None).await).await,
        json!([])
    );
    assert_eq!(
        send(
            &fixture.app,
            Method::GET,
            "/Environment/DirectoryContents",
            Credential::None,
            None,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    assert_validate_path_contract(fixture).await;
    assert_parent_default_and_drives(fixture).await;
}

async fn assert_validate_path_contract(fixture: &Fixture) {
    let directory = fixture.directory.path_string();
    let file = fixture
        .directory
        .path()
        .join("zeta.txt")
        .to_string_lossy()
        .into_owned();
    let missing = fixture
        .directory
        .path()
        .join("missing")
        .to_string_lossy()
        .into_owned();
    for (body, expected) in [
        (
            json!({ "Path": file, "IsFile": true }),
            StatusCode::NO_CONTENT,
        ),
        (
            json!({ "Path": directory, "IsFile": false }),
            StatusCode::NO_CONTENT,
        ),
        (
            json!({ "Path": file, "IsFile": false }),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({ "Path": directory, "IsFile": true }),
            StatusCode::NOT_FOUND,
        ),
        (json!({ "Path": missing }), StatusCode::NOT_FOUND),
        (json!({ "Path": null }), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            send(
                &fixture.app,
                Method::POST,
                "/Environment/ValidatePath",
                Credential::None,
                Some(body),
            )
            .await
            .status(),
            expected
        );
    }

    let before = child_names(fixture.directory.path());
    let response = send(
        &fixture.app,
        Method::POST,
        "/Environment/ValidatePath",
        Credential::None,
        Some(json!({
            "Path": fixture.directory.path_string(),
            "ValidateWritable": true
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(child_names(fixture.directory.path()), before);

    assert_eq!(
        send(
            &fixture.app,
            Method::POST,
            "/Environment/ValidatePath",
            Credential::None,
            None,
        )
        .await
        .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/Environment/ValidatePath")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

async fn assert_parent_default_and_drives(fixture: &Fixture) {
    let child = fixture.directory.path().join("媒体");
    let uri = format!(
        "/Environment/ParentPath?path={}",
        encoded(&child.to_string_lossy())
    );
    let response = send(&fixture.app, Method::GET, &uri, Credential::None, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, fixture.directory.path_string());

    let root = if cfg!(windows) { r"C:\" } else { "/" };
    let root_uri = format!("/Environment/ParentPath?path={}", encoded(root));
    let response = send(&fixture.app, Method::GET, &root_uri, Credential::None, None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(body_bytes(response).await.is_empty());

    let response = send(
        &fixture.app,
        Method::GET,
        "/Environment/DefaultDirectoryBrowser",
        Credential::None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, json!({}));

    let response = send(
        &fixture.app,
        Method::GET,
        "/Environment/Drives",
        Credential::None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let drives = body_json(response).await;
    assert!(!drives.as_array().unwrap().is_empty());
    assert!(drives.as_array().unwrap().iter().all(|drive| {
        drive["Type"] == "Directory" && PathBuf::from(drive["Path"].as_str().unwrap()).is_dir()
    }));
}

async fn assert_completed_setup_authorization(fixture: &Fixture) {
    let path = encoded(&fixture.directory.path_string());
    let child = encoded(&fixture.directory.path().join("媒体").to_string_lossy());
    let routes = [
        Route::get(
            format!("/Environment/DirectoryContents?path={path}"),
            StatusCode::OK,
        ),
        Route::post(
            "/Environment/ValidatePath".to_owned(),
            json!({ "Path": fixture.directory.path_string(), "IsFile": false }),
            StatusCode::NO_CONTENT,
        ),
        Route::get("/Environment/Drives".to_owned(), StatusCode::OK),
        Route::get(
            format!("/Environment/ParentPath?path={child}"),
            StatusCode::OK,
        ),
        Route::get(
            "/Environment/DefaultDirectoryBrowser".to_owned(),
            StatusCode::OK,
        ),
    ];

    for route in routes {
        assert_eq!(
            route.send(fixture, Credential::None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            route
                .send(fixture, Credential::Device(&fixture.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            route
                .send(fixture, Credential::Device(&fixture.admin_token))
                .await
                .status(),
            route.success
        );
        assert_eq!(
            route
                .send(fixture, Credential::ApiKeyHeader(&fixture.api_key_token))
                .await
                .status(),
            route.success
        );
        let separator = if route.uri.contains('?') { '&' } else { '?' };
        let api_key_uri = format!(
            "{}{separator}ApiKey={}",
            route.uri,
            encoded(&fixture.api_key_token)
        );
        assert_eq!(
            send(
                &fixture.app,
                route.method.clone(),
                &api_key_uri,
                Credential::None,
                route.body.clone(),
            )
            .await
            .status(),
            route.success
        );
    }

    assert_eq!(
        send(
            &fixture.app,
            Method::GET,
            "/Environment/DirectoryContents",
            Credential::None,
            None,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "authorization runs before query validation"
    );
}

async fn complete_startup(fixture: &Fixture) {
    assert_eq!(
        send(
            &fixture.app,
            Method::POST,
            "/Startup/Complete",
            Credential::None,
            None,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
    ApiKeyHeader(&'a str),
}

#[derive(Clone)]
struct Route {
    method: Method,
    uri: String,
    body: Option<Value>,
    success: StatusCode,
}

impl Route {
    fn get(uri: String, success: StatusCode) -> Self {
        Self {
            method: Method::GET,
            uri,
            body: None,
            success,
        }
    }

    fn post(uri: String, body: Value, success: StatusCode) -> Self {
        Self {
            method: Method::POST,
            uri,
            body: Some(body),
            success,
        }
    }

    async fn send(
        &self,
        fixture: &Fixture,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        send(
            &fixture.app,
            self.method.clone(),
            &self.uri,
            credential,
            self.body.clone(),
        )
        .await
    }
}

struct Fixture {
    app: axum::Router,
    directory: TestDirectory,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator("environment-admin")
            .await
            .expect("administrator creation");
        let user = users
            .create("environment-user")
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, "admin").await;
        let user_token = session(&devices, user.id, "user").await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create("environment-api-key")
            .await
            .expect("API key creation")
            .access_token;
        let directory = TestDirectory::new();
        fs::create_dir(directory.path().join("Zulu Folder")).unwrap();
        fs::create_dir(directory.path().join("媒体")).unwrap();
        fs::write(directory.path().join("zeta.txt"), b"zeta").unwrap();
        fs::write(directory.path().join("ä movie.mkv"), b"movie").unwrap();
        let app = jellyfin_api::router(AppState::new(
            database,
            "Environment Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            app,
            directory,
            admin_token,
            user_token,
            api_key_token,
        }
    }
}

async fn send(
    app: &axum::Router,
    method: Method,
    uri: &str,
    credential: Credential<'_>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    request = match credential {
        Credential::None => request,
        Credential::Device(token) => request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        ),
        Credential::ApiKeyHeader(token) => request.header("x-emby-token", token),
    };
    let body = if let Some(value) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&value).unwrap())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Environment Tests",
            "1.0",
            "PostgreSQL",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

async fn body_bytes(response: axum::response::Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), usize::MAX).await.unwrap()
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn child_names(path: &std::path::Path) -> Vec<String> {
    let mut names = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-environment-routes-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn path_string(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
