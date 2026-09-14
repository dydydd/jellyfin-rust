use std::path::PathBuf;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use chrono::{DateTime, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, EmbyCameraUploadRepository, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde::Deserialize;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Camera Tests\", DeviceId=\"request-device\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_camera_";

#[tokio::test]
async fn camera_uploads_are_streamed_persisted_and_device_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");
    let storage_root =
        std::env::temp_dir().join(format!("emby-camera-{}", Uuid::new_v4().simple()));

    let task_database_name = database_name.clone();
    let task_storage_root = storage_root.clone();
    let outcome =
        tokio::spawn(async move { exercise(&task_database_name, task_storage_root).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if storage_root.starts_with(std::env::temp_dir()) {
        let _ = tokio::fs::remove_dir_all(&storage_root).await;
    }
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str, storage_root: PathBuf) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database).await.expect("migrations");
    let fixture = Fixture::new(database.clone(), storage_root).await;
    fixture.assert_authorization_and_binding().await;
    fixture.assert_raw_upload_and_shared_device_history().await;
    fixture.assert_multipart_and_restart().await;
    fixture.assert_protocol_isolation().await;
    drop(fixture);
    database.close().await.expect("database close");
}

struct Fixture {
    database: DatabaseConnection,
    storage_root: PathBuf,
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    first_token: String,
    second_token: String,
    denied_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection, storage_root: PathBuf) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("camera-admin-{suffix}"))
            .await
            .expect("admin");
        let first = users
            .create(&format!("camera-first-{suffix}"))
            .await
            .expect("first user");
        let second = users
            .create(&format!("camera-second-{suffix}"))
            .await
            .expect("second user");
        let denied = users
            .create(&format!("camera-denied-{suffix}"))
            .await
            .expect("denied user");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, "admin-device", "Admin").await;
        let first_token = session(&devices, first.id, "Shared-Camera", "../../Phone One").await;
        let second_token = session(&devices, second.id, "shared-camera", "Phone Two").await;
        let denied_token = session(&devices, denied.id, "denied-camera", "Denied").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("camera-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = state(database.clone(), &storage_root);
        let emby = jellyfin_emby_api::router(state.clone());
        let jellyfin = jellyfin_api::router(state);
        for user_id in [first.id, second.id] {
            let response = request(
                &emby,
                Method::POST,
                &format!("/emby/Users/{user_id}/Policy"),
                Some(&admin_token),
                Some("application/json"),
                Body::from(json!({"AllowCameraUpload": true}).to_string()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "enable camera upload");
        }
        Self {
            database,
            storage_root,
            emby,
            jellyfin,
            admin_token,
            first_token,
            second_token,
            denied_token,
            api_key,
        }
    }

    async fn assert_authorization_and_binding(&self) {
        let path = "/emby/dEvIcEs/cAmErAuPlOaDs";
        assert_eq!(
            request(&self.emby, Method::GET, path, None, None, Body::empty())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&self.emby, Method::POST, path, None, None, Body::from("x"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                path,
                Some(&self.denied_token),
                None,
                Body::from("x")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
            "camera role must precede query/body binding"
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                path,
                Some(&self.api_key),
                None,
                Body::from("x")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "API keys are role-exempt but still require the generated query"
        );
        let api_key_history = request(
            &self.emby,
            Method::GET,
            path,
            Some(&self.api_key),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(api_key_history.status(), StatusCode::OK);
        let api_key_history: UploadHistory =
            serde_json::from_slice(&response_bytes(api_key_history).await)
                .expect("API-key device history");
        assert_eq!(api_key_history.device_id, "request-device");
        assert!(api_key_history.files_uploaded.is_empty());
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                path,
                Some(&self.first_token),
                None,
                Body::from("x")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/Devices/CameraUploads?Album=a&Name=n&Id=i&DateCreated=bad",
                Some(&self.first_token),
                None,
                Body::from("x")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/Devices/CameraUploads?Album=a&Name=n&Id=i",
                Some(&self.first_token),
                Some("image/jpeg"),
                Body::empty()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert!(
            EmbyCameraUploadRepository::new(self.database.clone())
                .list("Shared-Camera")
                .await
                .expect("empty history")
                .is_empty()
        );
    }

    async fn assert_raw_upload_and_shared_device_history(&self) {
        let path = "/emby/dEvIcEs/cAmErAuPlOaDs?Album=old&aLbUm=..%2F..%2FVacation&Name=old.jpg&nAmE=..%2Fphoto.png&Id=first&iD=raw-id&datecreated=2026-09-14T01%3A02%3A03%2B08%3A00";
        let response = request(
            &self.emby,
            Method::POST,
            path,
            Some(&self.first_token),
            Some("image/png"),
            Body::from("raw-camera-bytes"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_bytes(response).await.is_empty());

        let response = request(
            &self.emby,
            Method::GET,
            "/emby/Devices/CameraUploads",
            Some(&self.second_token),
            None,
            Body::empty(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let history: UploadHistory = serde_json::from_slice(&response_bytes(response).await)
            .expect("Swift-decodable history");
        assert_eq!(history.device_id, "shared-camera");
        assert_eq!(history.files_uploaded.len(), 1);
        let file = &history.files_uploaded[0];
        assert_eq!(file.name, "../photo.png");
        assert_eq!(file.id, "raw-id");
        assert_eq!(file.album, "../../Vacation");
        assert_eq!(file.mime_type.as_deref(), Some("image/png"));
        assert_eq!(
            file.date_created,
            Some(
                DateTime::parse_from_rfc3339("2026-09-14T01:02:03+08:00")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );

        let stored = EmbyCameraUploadRepository::new(self.database.clone())
            .list("SHARED-CAMERA")
            .await
            .expect("stored history");
        assert_eq!(stored.len(), 1);
        let stored_path = PathBuf::from(&stored[0].stored_path);
        let root = tokio::fs::canonicalize(self.storage_root.join("programdata/camera-uploads"))
            .await
            .expect("camera root");
        let stored_path = tokio::fs::canonicalize(stored_path)
            .await
            .expect("stored upload");
        assert!(stored_path.starts_with(root));
        assert_eq!(
            tokio::fs::read(stored_path).await.expect("stored bytes"),
            b"raw-camera-bytes"
        );
    }

    async fn assert_multipart_and_restart(&self) {
        let boundary = "emby-camera-boundary";
        let multipart = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nignored\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"camera.jpg\"\r\nContent-Type: image/jpeg\r\n\r\nmultipart-camera-bytes\r\n--{boundary}--\r\n"
        );
        let response = request(
            &self.emby,
            Method::POST,
            "/emby/Devices/CameraUploads?ALBUM=&NAME=camera.jpg&ID=multipart-id",
            Some(&self.second_token),
            Some(&format!("multipart/form-data; boundary={boundary}")),
            Body::from(multipart),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let restarted = jellyfin_emby_api::router(state(self.database.clone(), &self.storage_root));
        let response = request(
            &restarted,
            Method::GET,
            "/emby/Devices/CameraUploads",
            Some(&self.first_token),
            None,
            Body::empty(),
        )
        .await;
        let history: UploadHistory =
            serde_json::from_slice(&response_bytes(response).await).expect("restarted history");
        assert_eq!(history.files_uploaded.len(), 2);
        assert_eq!(history.files_uploaded[1].id, "multipart-id");
        assert_eq!(
            history.files_uploaded[1].mime_type.as_deref(),
            Some("image/jpeg")
        );
    }

    async fn assert_protocol_isolation(&self) {
        for path in ["/Devices/CameraUploads", "/api/Devices/CameraUploads"] {
            for method in [Method::GET, Method::POST] {
                assert_eq!(
                    request(
                        &self.jellyfin,
                        method,
                        path,
                        Some(&self.admin_token),
                        Some("image/jpeg"),
                        Body::from("x")
                    )
                    .await
                    .status(),
                    StatusCode::NOT_FOUND,
                    "{path}"
                );
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UploadHistory {
    device_id: String,
    files_uploaded: Vec<UploadedFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UploadedFile {
    name: String,
    id: String,
    album: String,
    mime_type: Option<String>,
    date_created: Option<DateTime<Utc>>,
}

fn state(database: DatabaseConnection, storage_root: &std::path::Path) -> AppState {
    AppState::new(
        database,
        "Emby Camera Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    )
    .with_storage_paths(
        storage_root.join("programdata"),
        storage_root.join("web"),
        storage_root.join("cache/images"),
        storage_root.join("cache"),
        storage_root.join("metadata"),
    )
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: Body,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    app.clone()
        .oneshot(request.body(body).expect("camera request"))
        .await
        .expect("camera response")
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .expect("bounded response")
        .to_vec()
}

async fn session(
    devices: &DeviceRepository,
    user_id: Uuid,
    device_id: &str,
    device_name: &str,
) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Camera Tests",
            "1.0",
            device_name,
            device_id,
        ))
        .await
        .expect("session")
        .access_token
}
