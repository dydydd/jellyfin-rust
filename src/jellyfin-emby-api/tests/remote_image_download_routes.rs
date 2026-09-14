#![allow(clippy::too_many_lines)]

use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemImageRepository, BaseItemImageType, BaseItemRepository,
    DatabaseConfig, DeviceRepository, NewBaseItem, NewBaseItemImage, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Remote Image Tests\", DeviceId=\"emby-remote-images\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_remote_image_";

#[tokio::test]
async fn emby_remote_image_download_uses_its_generated_body_contract() {
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
        panic!("temporary database task was cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");
    let fixture = Fixture::new(database.clone()).await;

    fixture.assert_binding_and_destination_ordinal().await;
    fixture
        .assert_authorization_and_validation_precedence()
        .await;
    fixture.assert_protocol_isolation().await;

    drop(fixture);
    database.close().await.expect("database cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    user_token: String,
    api_key: String,
    item_id: Uuid,
    _temporary: TempDirectory,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let temporary = TempDirectory::new();
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("remote-image-admin-{suffix}"))
            .await
            .expect("administrator");
        let user = users
            .create(&format!("remote-image-user-{suffix}"))
            .await
            .expect("ordinary user");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("remote-image-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some("Emby remote image movie".to_owned());
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/remote-image-{suffix}.mkv"));
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("movie");

        let images = BaseItemImageRepository::new(database.clone());
        for (position, bytes) in [(0, b"old-zero".as_slice()), (1, b"old-one".as_slice())] {
            let path = temporary.path().join(format!("backdrop-{position}.jpg"));
            std::fs::write(&path, bytes).expect("seed image");
            images
                .set_or_append(
                    item.id,
                    NewBaseItemImage {
                        image_type: BaseItemImageType::Backdrop,
                        image_index: 0,
                        path: path.to_string_lossy().into_owned(),
                        date_modified: Utc::now(),
                        width: None,
                        height: None,
                        blurhash: None,
                    },
                )
                .await
                .expect("seed image metadata");
        }

        let state = AppState::new(
            database.clone(),
            "Emby Remote Image Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            temporary.path().join("programdata"),
            temporary.path().join("web"),
            temporary.path().join("cache/images"),
            temporary.path().join("cache"),
            temporary.path().join("metadata"),
        );
        Self {
            database,
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            admin_token,
            user_token,
            api_key,
            item_id: item.id,
            _temporary: temporary,
        }
    }

    async fn assert_binding_and_destination_ordinal(&self) {
        let (url, upstream) = image_server(b"replacement-zero").await;
        let route = format!(
            "/emby/iTeMs/{}/rEmOtEiMaGeS/dOwNlOaD?TYPE=Primary&tYpE=Backdrop&PROVIDERNAME=first&pRoViDeRnAmE=last&IMAGEURL=http%3A%2F%2F127.0.0.1%3A1%2Fbad&iMaGeUrL={}",
            self.item_id,
            encode(&url),
        );
        let response = request(
            &self.emby,
            Method::POST,
            &route,
            Some(&self.admin_token),
            Some(r#"{"IMAGEINDEX":1,"iMaGeInDeX":0}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        upstream.await.expect("replacement upstream");

        let backdrops = self.backdrops().await;
        assert_eq!(backdrops.len(), 2);
        assert_eq!(
            tokio::fs::read(&backdrops[0].path).await.unwrap(),
            b"replacement-zero"
        );
        assert_eq!(
            tokio::fs::read(&backdrops[1].path).await.unwrap(),
            b"old-one"
        );

        for (body, bytes, token) in [
            (
                r#"{"ImageIndex":null}"#,
                b"null-appends".as_slice(),
                &self.api_key,
            ),
            (
                r#"{"ImageIndex":-1}"#,
                b"negative-appends".as_slice(),
                &self.admin_token,
            ),
        ] {
            let (url, upstream) = image_server(bytes).await;
            let route = format!(
                "/emby/Items/{}/RemoteImages/Download?Type=Backdrop&ImageUrl={}",
                self.item_id,
                encode(&url),
            );
            let response = request(&self.emby, Method::POST, &route, Some(token), Some(body)).await;
            assert_eq!(response.status(), StatusCode::OK, "{body}");
            upstream.await.expect("append upstream");
        }
        let backdrops = self.backdrops().await;
        assert_eq!(backdrops.len(), 4);
        assert_eq!(
            tokio::fs::read(&backdrops[2].path).await.unwrap(),
            b"null-appends"
        );
        assert_eq!(
            tokio::fs::read(&backdrops[3].path).await.unwrap(),
            b"negative-appends"
        );
    }

    async fn assert_authorization_and_validation_precedence(&self) {
        let malformed = format!(
            "/emby/Items/{}/RemoteImages/Download?Type=invalid",
            Uuid::new_v4()
        );
        assert_eq!(
            request(&self.emby, Method::POST, &malformed, None, Some("not-json"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &malformed,
                Some(&self.user_token),
                Some("not-json"),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
        );

        let route = format!(
            "/emby/Items/{}/RemoteImages/Download?Type=Primary",
            self.item_id
        );
        for body in [None, Some("null"), Some(r#"{"ImageIndex":2147483648}"#)] {
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    &route,
                    Some(&self.admin_token),
                    body,
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "body {body:?}",
            );
        }
        for query in ["", "?Type=%20%20"] {
            let route = format!("/emby/Items/{}/RemoteImages/Download{query}", self.item_id);
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    &route,
                    Some(&self.admin_token),
                    Some("{}"),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
            );
        }
    }

    async fn assert_protocol_isolation(&self) {
        for prefix in ["", "/api"] {
            let (url, upstream) = image_server(b"jellyfin-primary").await;
            let route = format!(
                "{prefix}/Items/{}/RemoteImages/Download?type=Primary&imageUrl={}",
                self.item_id,
                encode(&url),
            );
            let response = request(
                &self.jellyfin,
                Method::POST,
                &route,
                Some(&self.admin_token),
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{prefix}");
            upstream.await.expect("Jellyfin upstream");
        }

        let emby = format!(
            "/emby/Items/{}/RemoteImages/Download?Type=Primary&ImageUrl=http%3A%2F%2F127.0.0.1%3A1%2Funused",
            self.item_id
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &emby,
                Some(&self.admin_token),
                None,
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
        );
    }

    async fn backdrops(&self) -> Vec<jellyfin_data::BaseItemImage> {
        BaseItemImageRepository::new(self.database.clone())
            .list(self.item_id)
            .await
            .expect("image list")
            .into_iter()
            .filter(|image| image.image_type == BaseItemImageType::Backdrop)
            .collect()
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Remote Image Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("device session")
        .access_token
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn image_server(bytes: &'static [u8]) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("image upstream bind");
    let address = listener.local_addr().expect("image upstream address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("image upstream accept");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).await.expect("image request");
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        stream
            .write_all(headers.as_bytes())
            .await
            .expect("image response headers");
        stream.write_all(bytes).await.expect("image response body");
    });
    (format!("http://{address}/image.jpg"), task)
}

fn encode(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-emby-remote-images-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
