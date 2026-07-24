use std::path::{Path, PathBuf};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewTrickplayInfo, TrickplayInfoRepository,
};
use jellyfin_model::UserPolicy;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Trickplay Tests\", Device=\"Test\", DeviceId=\"trickplay\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_trickplay_routes_";
const JPEG: &[u8] = b"\xff\xd8trickplay-jpeg\xff\xd9";

#[tokio::test]
async fn trickplay_routes_match_official_playlist_auth_visibility_and_file_contract() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");
    let storage_root = std::env::temp_dir().join(format!("jellyfin-trickplay-{database_name}"));

    let task_database_name = database_name.clone();
    let task_storage_root = storage_root.clone();
    let outcome = tokio::spawn(async move {
        exercise_trickplay_routes(&task_database_name, task_storage_root).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    let _ = tokio::fs::remove_dir_all(storage_root).await;
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_trickplay_routes(database_name: &str, storage_root: PathBuf) {
    let fixture = Fixture::new(database_name, storage_root).await;
    assert_authentication_contract(&fixture).await;
    assert_exact_playlist_and_media_source_override(&fixture).await;
    assert_tile_visibility_and_file_contract(&fixture).await;
    assert_missing_and_invalid_resources(&fixture).await;
    fixture.database.close().await.unwrap();
}

async fn assert_authentication_contract(fixture: &Fixture) {
    let playlist = Fixture::playlist_route(fixture.primary_id, 320);
    let tile = Fixture::tile_route(fixture.primary_id, 320, 0);
    for route in [&playlist, &tile] {
        assert_eq!(
            fixture
                .request(Method::GET, route, Credential::None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            fixture
                .request(Method::GET, route, Credential::Device("bad-token"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}

async fn assert_exact_playlist_and_media_source_override(fixture: &Fixture) {
    let route = Fixture::playlist_route(fixture.primary_id, 320);
    let response = fixture
        .request(Method::GET, &route, Credential::Device(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-mpegURL; charset=utf-8"
    );
    assert_eq!(
        body_string(response).await,
        expected_primary_playlist(fixture.primary_id, &fixture.user_token)
    );

    let override_route = format!(
        "{}?MediaSourceId={}",
        Fixture::playlist_route(fixture.primary_id, 640),
        fixture.alternate_id
    );
    let response = fixture
        .request(
            Method::GET,
            &override_route,
            Credential::Device(&fixture.user_token),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_string(response).await,
        expected_alternate_playlist(fixture.alternate_id, &fixture.user_token)
    );

    // The official playlist lookup is metadata-only and does not apply item visibility.
    let hidden_route = Fixture::playlist_route(fixture.hidden_id, 320);
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &hidden_route,
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::OK
    );
}

async fn assert_tile_visibility_and_file_contract(fixture: &Fixture) {
    let route = Fixture::tile_route(fixture.primary_id, 320, 0);
    let response = fixture
        .request(Method::GET, &route, Credential::Device(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment"
    );
    assert_eq!(body_bytes(response).await, Bytes::from_static(JPEG));

    let head = fixture
        .request(
            Method::HEAD,
            &route,
            Credential::Device(&fixture.user_token),
        )
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert!(body_bytes(head).await.is_empty());

    let hidden_route = Fixture::tile_route(fixture.hidden_id, 320, 0);
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &hidden_route,
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let api_key_route = format!("{hidden_route}?ApiKey={}", fixture.api_key);
    let response = fixture
        .request(Method::GET, &api_key_route, Credential::None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, Bytes::from_static(JPEG));

    let override_route = format!(
        "{}?mediaSourceId={}",
        Fixture::tile_route(fixture.primary_id, 640, 0),
        fixture.alternate_id
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &override_route,
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::OK
    );
}

async fn assert_missing_and_invalid_resources(fixture: &Fixture) {
    for route in [
        Fixture::playlist_route(fixture.primary_id, 999),
        Fixture::playlist_route(fixture.empty_id, 320),
        Fixture::tile_route(fixture.primary_id, 999, 0),
        Fixture::tile_route(fixture.primary_id, 320, 99),
        Fixture::tile_route(Uuid::new_v4(), 320, 0),
    ] {
        assert_eq!(
            fixture
                .request(Method::GET, &route, Credential::Device(&fixture.user_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "missing resource must return 404 for {route}"
        );
    }

    for route in [
        format!(
            "/Videos/{}/Trickplay/not-a-width/tiles.m3u8",
            fixture.primary_id
        ),
        format!(
            "/Videos/{}/Trickplay/320/not-an-index.jpg",
            fixture.primary_id
        ),
        format!(
            "/Videos/{}/Trickplay/320/tiles.m3u8?MediaSourceId=invalid",
            fixture.primary_id
        ),
    ] {
        assert_eq!(
            fixture
                .request(Method::GET, &route, Credential::Device(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    user_token: String,
    api_key: String,
    primary_id: Uuid,
    alternate_id: Uuid,
    hidden_id: Uuid,
    empty_id: Uuid,
}

impl Fixture {
    async fn new(database_name: &str, storage_root: PathBuf) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 8,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database).await.unwrap();

        let users = UserService::new(database.clone());
        let user = users.create("trickplay-user").await.unwrap();
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.unwrap();
        let allowed_folder =
            create_item(&items, "CollectionFolder", Some(root.id), "Allowed").await;
        let hidden_folder = create_item(&items, "CollectionFolder", Some(root.id), "Hidden").await;
        let primary = create_item(&items, "Video", Some(allowed_folder.id), "Primary").await;
        let alternate = create_item(&items, "Video", Some(allowed_folder.id), "Alternate").await;
        let hidden = create_item(&items, "Video", Some(hidden_folder.id), "Hidden Video").await;
        let empty = create_item(&items, "Video", Some(allowed_folder.id), "Empty").await;

        let policy = UserPolicy {
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            enable_all_folders: false,
            enabled_folders: vec![allowed_folder.id],
            ..UserPolicy::default()
        };
        users.update_policy(user.id, &policy).await.unwrap();
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Trickplay Tests",
                "1.0",
                "Test",
                "trickplay-device",
            ))
            .await
            .unwrap()
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create("trickplay-api-key")
            .await
            .unwrap()
            .access_token;

        let trickplay = TrickplayInfoRepository::new(database.clone());
        trickplay
            .upsert(primary.id, info(320, 180, 2, 2, 6, 1_500))
            .await
            .unwrap();
        trickplay
            .upsert(alternate.id, info(640, 360, 3, 2, 7, 750))
            .await
            .unwrap();
        trickplay
            .upsert(hidden.id, info(320, 180, 1, 1, 1, 1_000))
            .await
            .unwrap();
        trickplay
            .upsert(empty.id, info(320, 180, 2, 2, 0, 1_500))
            .await
            .unwrap();

        let program_data = storage_root.join("programdata");
        for (item_id, width, tile_width, tile_height) in [
            (primary.id, 320, 2, 2),
            (alternate.id, 640, 3, 2),
            (hidden.id, 320, 1, 1),
        ] {
            let tile = tile_path(
                &program_data.join("trickplay"),
                item_id,
                width,
                tile_width,
                tile_height,
                0,
            );
            tokio::fs::create_dir_all(tile.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(tile, JPEG).await.unwrap();
        }

        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Trickplay Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                &program_data,
                storage_root.join("web"),
                storage_root.join("images"),
                storage_root.join("cache"),
                storage_root.join("metadata"),
            ),
        );
        Self {
            database,
            app,
            user_token,
            api_key,
            primary_id: primary.id,
            alternate_id: alternate.id,
            hidden_id: hidden.id,
            empty_id: empty.id,
        }
    }

    fn playlist_route(item_id: Uuid, width: i32) -> String {
        format!("/Videos/{item_id}/Trickplay/{width}/tiles.m3u8")
    }

    fn tile_route(item_id: Uuid, width: i32, index: i32) -> String {
        format!("/Videos/{item_id}/Trickplay/{width}/{index}.jpg")
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Credential::Device(token) = credential {
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
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = parent_id;
    item.name = Some(name.to_owned());
    if item_type == "CollectionFolder" {
        item.is_folder = true;
    } else {
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/{name}.mkv"));
    }
    repository.create(item).await.unwrap()
}

const fn info(
    width: i32,
    height: i32,
    tile_width: i32,
    tile_height: i32,
    thumbnail_count: i32,
    interval: i32,
) -> NewTrickplayInfo {
    NewTrickplayInfo {
        width,
        height,
        tile_width,
        tile_height,
        thumbnail_count,
        interval,
        bandwidth: 22_000,
    }
}

fn tile_path(
    root: &Path,
    item_id: Uuid,
    width: i32,
    tile_width: i32,
    tile_height: i32,
    index: i32,
) -> PathBuf {
    let id = item_id.hyphenated().to_string();
    root.join(&id[..2])
        .join(id)
        .join(format!("{width} - {tile_width}x{tile_height}"))
        .join(format!("{index}.jpg"))
}

fn expected_primary_playlist(item_id: Uuid, token: &str) -> String {
    format!(
        "#EXTM3U\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-VERSION:7\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXT-X-IMAGES-ONLY\n\
#EXTINF:6,\n\
#EXT-X-TILES:RESOLUTION=320x180,LAYOUT=2x2,DURATION=1.5\n\
0.jpg?MediaSourceId={}&ApiKey={token}\n\
#EXTINF:3,\n\
#EXT-X-TILES:RESOLUTION=320x180,LAYOUT=2x2,DURATION=1.5\n\
1.jpg?MediaSourceId={}&ApiKey={token}\n\
#EXT-X-ENDLIST\n",
        item_id.simple(),
        item_id.simple()
    )
}

fn expected_alternate_playlist(item_id: Uuid, token: &str) -> String {
    format!(
        "#EXTM3U\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-VERSION:7\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXT-X-IMAGES-ONLY\n\
#EXTINF:4.5,\n\
#EXT-X-TILES:RESOLUTION=640x360,LAYOUT=3x2,DURATION=0.75\n\
0.jpg?MediaSourceId={}&ApiKey={token}\n\
#EXTINF:0.75,\n\
#EXT-X-TILES:RESOLUTION=640x360,LAYOUT=3x2,DURATION=0.75\n\
1.jpg?MediaSourceId={}&ApiKey={token}\n\
#EXT-X-ENDLIST\n",
        item_id.simple(),
        item_id.simple()
    )
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    to_bytes(response.into_body(), usize::MAX).await.unwrap()
}

async fn body_string(response: axum::response::Response) -> String {
    String::from_utf8(body_bytes(response).await.to_vec()).unwrap()
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
