use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, tuner_host, user},
};
use jellyfin_live_tv::tuner_hosts::TunerHostManager;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn official_tuner_host_contract_and_postgres_lifecycle() {
    let fixture = Fixture::new().await;
    assert_post_rejections(&fixture).await;
    assert_live_tv_route_surface(&fixture).await;
    let created_id = assert_local_create_update_and_persistence(&fixture).await;
    let api_key_id = assert_http_api_key_create(&fixture).await;
    assert_delete_contract(&fixture, &created_id).await;
    fixture.cleanup(&[&api_key_id]).await;
}

async fn assert_live_tv_route_surface(fixture: &Fixture) {
    for route in [
        "/LiveTv/Info",
        "/LiveTv/Channels",
        "/LiveTv/Recordings",
        "/LiveTv/Recordings/Series",
        "/LiveTv/Recordings/Groups",
        "/LiveTv/Recordings/Folders",
        "/LiveTv/Timers",
        "/LiveTv/Timers/Defaults",
        "/LiveTv/Programs",
        "/LiveTv/Programs/Recommended",
        "/LiveTv/SeriesTimers",
        "/LiveTv/ListingProviders/Default",
        "/LiveTv/TunerHosts/Types",
        "/LiveTv/ChannelMappingOptions",
        "/LiveTv/Tuners/Discover",
        "/LiveTv/ListingProviders/SchedulesDirect/Countries",
    ] {
        assert_eq!(
            fixture
                .get(route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::OK,
            "{route}"
        );
    }
    for route in [
        "/LiveTv/Channels/00000000-0000-0000-0000-000000000000",
        "/LiveTv/Recordings/00000000-0000-0000-0000-000000000000",
        "/LiveTv/Timers/not-found",
        "/LiveTv/Programs/not-found",
        "/LiveTv/SeriesTimers/not-found",
    ] {
        assert_eq!(
            fixture
                .get(route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{route}"
        );
    }
    assert_eq!(
        fixture
            .post_uri(
                "/LiveTv/ChannelMappings",
                Some(&fixture.admin_token),
                json!({}),
            )
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .post_uri("/LiveTv/Timers", Some(&fixture.admin_token), json!({}))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

async fn assert_post_rejections(fixture: &Fixture) {
    let valid_body = json!({ "Type": "m3u", "Url": fixture.playlist });
    assert_eq!(
        fixture.post(None, valid_body.clone()).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post(Some(&fixture.user_token), valid_body.clone())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .post(
                Some(&fixture.admin_token),
                json!({ "Type": "unknown", "Url": fixture.playlist }),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .post(
                Some(&fixture.admin_token),
                json!({
                    "Type": "m3u",
                    "Url": fixture.playlist,
                    "TunerCount": -1
                }),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .post(
                Some(&fixture.admin_token),
                json!({
                    "Type": "m3u",
                    "Url": fixture.playlist.with_extension("missing")
                }),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_local_create_update_and_persistence(fixture: &Fixture) -> String {
    let valid_body = json!({ "Type": "m3u", "Url": fixture.playlist });
    let response = fixture.post(Some(&fixture.admin_token), valid_body).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let created = body_json(response).await;
    assert_eq!(created["Type"], "m3u");
    assert_eq!(created["Url"], fixture.playlist.to_string_lossy().as_ref());
    assert_eq!(created["AllowHWTranscoding"], true);
    assert_eq!(created["AllowStreamSharing"], true);
    assert_eq!(created["IgnoreDts"], true);
    assert_eq!(created["FallbackMaxStreamingBitrate"], 30_000_000);
    let created_id = created["Id"].as_str().expect("created ID").to_owned();
    assert_eq!(created_id.len(), 32);
    assert!(created_id.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let persisted = TunerHostManager::new(fixture.database.clone())
        .list()
        .await
        .expect("cross-instance tuner-host list");
    assert!(
        persisted
            .iter()
            .any(|host| host.id.as_deref() == Some(created_id.as_str()))
    );

    let update_response = fixture
        .post(
            Some(&fixture.admin_token),
            json!({
                "Id": &created_id,
                "Type": "M3U",
                "Url": fixture.playlist,
                "FriendlyName": "Updated tuner"
            }),
        )
        .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let updated = body_json(update_response).await;
    assert_eq!(updated["Id"], created_id);
    assert_eq!(updated["Type"], "M3U");
    assert_eq!(updated["FriendlyName"], "Updated tuner");
    created_id
}

async fn assert_http_api_key_create(fixture: &Fixture) -> String {
    let (http_url, server) = one_shot_playlist_server();
    let api_key_response = fixture
        .post(
            Some(&fixture.api_key_token),
            json!({ "Type": "m3u", "Url": http_url }),
        )
        .await;
    server.join().expect("playlist server thread");
    assert_eq!(api_key_response.status(), StatusCode::OK);
    let api_key_host = body_json(api_key_response).await;
    api_key_host["Id"]
        .as_str()
        .expect("API key host ID")
        .to_owned()
}

async fn assert_delete_contract(fixture: &Fixture, created_id: &str) {
    assert_eq!(
        fixture.delete("/LiveTv/TunerHosts", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .delete("/LiveTv/TunerHosts", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for uri in [
        "/LiveTv/TunerHosts",
        "/LiveTv/TunerHosts?id=",
        "/LiveTv/TunerHosts?id=not-a-compact-uuid",
    ] {
        assert_eq!(
            fixture
                .delete(uri, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        fixture
            .delete(
                &format!("/LiveTv/TunerHosts?id={created_id}"),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        TunerHostManager::new(fixture.database.clone())
            .list()
            .await
            .expect("post-delete tuner-host list")
            .iter()
            .all(|host| host.id.as_deref() != Some(created_id))
    );
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    playlist: PathBuf,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
    api_key_id: i64,
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
            .create(&format!("tuner-admin-{suffix}"))
            .await
            .expect("admin user creation");
        let mut active = admin.clone().into_active_model();
        active.is_administrator = Set(true);
        let admin = active.update(&database).await.expect("admin elevation");
        let user = users
            .create(&format!("tuner-user-{suffix}"))
            .await
            .expect("ordinary user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("tuner-key-{suffix}"))
            .await
            .expect("API key creation");
        let playlist = std::env::temp_dir().join(format!("jellyfin-rust-{suffix}.m3u8"));
        tokio::fs::write(&playlist, b"#EXTM3U\n")
            .await
            .expect("test playlist creation");
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Tuner Host Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            playlist,
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            api_key_token: api_key.access_token,
            api_key_id: api_key.id,
        }
    }

    async fn post(&self, token: Option<&str>, body: Value) -> axum::response::Response {
        self.post_uri("/LiveTv/TunerHosts", token, body).await
    }

    async fn post_uri(
        &self,
        uri: &str,
        token: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header("x-emby-token", token);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().method(Method::GET).uri(uri);
        if let Some(token) = token {
            request = request.header("x-emby-token", token);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn delete(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().method(Method::DELETE).uri(uri);
        if let Some(token) = token {
            request = request.header("x-emby-token", token);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self, tuner_ids: &[&str]) {
        let ids = tuner_ids
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect::<Vec<_>>();
        tuner_host::Entity::delete_many()
            .filter(tuner_host::Column::Id.is_in(ids))
            .exec(&self.database)
            .await
            .expect("test tuner cleanup");
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("test API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
        tokio::fs::remove_file(self.playlist)
            .await
            .expect("test playlist cleanup");
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Tuner Host Tests",
            "1.0",
            "Test Device",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

fn one_shot_playlist_server() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("playlist listener");
    let address = listener.local_addr().expect("playlist listener address");
    listener
        .set_nonblocking(true)
        .expect("nonblocking playlist listener");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "playlist request timed out");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("playlist request failed: {error}"),
            }
        };
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).expect("read playlist request");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: audio/x-mpegurl\r\nContent-Length: 8\r\nConnection: close\r\n\r\n#EXTM3U\n",
            )
            .expect("write playlist response");
    });
    (format!("http://{address}/playlist.m3u8"), handle)
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}
