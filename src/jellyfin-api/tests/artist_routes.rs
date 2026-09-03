#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewUserData,
    UserDataRepository,
    entities::{base_item, item_value},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Artist Tests\", DeviceId=\"artist-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_artist_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn artist_routes_match_official_artist_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/Artists", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/Artists/ABBA", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let artists = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists?limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(
        &artists,
        &[&fixture.alpha_artist, &fixture.beta_artist],
        4,
        0,
    );

    let descending = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists?sortBy=SortName&sortOrder=Descending&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(
        &descending,
        &[&fixture.movie_artist, &fixture.gamma_artist],
        4,
        0,
    );

    let album_artists = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists/AlbumArtists",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(
        &album_artists,
        &[&fixture.album_artist, &fixture.second_album_artist],
        2,
        0,
    );

    let searched = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Artists?searchTerm={}", encoded(&fixture.beta_artist)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(&searched, &[&fixture.beta_artist], 1, 0);

    let media_filtered = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists?mediaTypes=Video",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(
        &media_filtered,
        &[&fixture.beta_artist, &fixture.movie_artist],
        2,
        0,
    );

    let favorite = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists?isFavorite=true",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(&favorite, &[&fixture.beta_artist], 1, 0);

    let folder_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Artists?parentId={}", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(&folder_scoped, &[&fixture.gamma_artist], 1, 0);

    let item_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Artists?parentId={}", fixture.audio_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_artists(&item_scoped, &[&fixture.alpha_artist], 1, 0);

    let no_total = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists?limit=1&enableTotalRecordCount=false",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(no_total["Items"].as_array().expect("items").len(), 1);
    assert_eq!(no_total["TotalRecordCount"], 1);

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Artists?sortOrder=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Artists?sortBy=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let artist = body_json(
        fixture
            .request(
                Method::GET,
                &artist_route(&fixture.alpha_artist),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(artist["Id"], fixture.alpha_artist_id.simple().to_string());
    assert_eq!(artist["Name"], fixture.alpha_artist);
    assert_eq!(artist["Type"], "MusicArtist");
    assert_eq!(
        artist["PresentationUniqueKey"],
        format!("Artist-{}", fixture.alpha_artist)
    );
    assert!(artist.get("item_type").is_none());

    let missing = body_json(
        fixture
            .request(
                Method::GET,
                "/Artists/Missing%20Artist",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing["Name"], "Missing Artist");
    assert_eq!(missing["Type"], "MusicArtist");
    assert_eq!(missing["PresentationUniqueKey"], "Artist-Missing Artist");
    assert_ne!(missing["Id"], fixture.alpha_artist_id.simple().to_string());

    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Artists?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let admin_targeted = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Artists?userId={}", fixture.user_id),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_targeted["TotalRecordCount"], 4);

    fixture.cleanup().await;
}

#[tokio::test]
async fn artist_image_route_resolves_public_base_item_owner() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let image_name = format!("Image Artist {}", Uuid::new_v4().simple());
    let linked = ItemValueRepository::new(fixture.database.clone())
        .link(
            fixture.audio_id,
            item_value::ItemValueType::Artist,
            &image_name,
        )
        .await
        .unwrap();
    let artist = create_item(&items, "MusicArtist", &image_name, None, true, "").await;
    assert_ne!(linked.item_value_id, artist.id);
    let path = std::env::temp_dir().join(format!(
        "jellyfin-artist-image-{}.png",
        Uuid::new_v4().simple()
    ));
    let image = image::RgbaImage::from_pixel(4, 2, image::Rgba([10, 80, 220, 255]));
    image.save(&path).unwrap();
    BaseItemImageRepository::new(fixture.database.clone())
        .replace(
            artist.id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Primary,
                image_index: 0,
                path: path.to_string_lossy().into_owned(),
                date_modified: Utc::now(),
                width: Some(4),
                height: Some(2),
                blurhash: None,
            }],
        )
        .await
        .unwrap();
    let route = format!(
        "/Artists/{}/Images/Primary/0?tag=artist-tag",
        encoded(&image_name)
    );
    assert_eq!(
        fixture
            .request(Method::GET, &route, Credential::Device("invalid-token"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = fixture.request(Method::GET, &route, Credential::None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(response.headers()[header::ETAG], "\"artist-tag\"");
    let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 2));
    let head = fixture
        .request(Method::HEAD, &route, Credential::None)
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(
        to_bytes(head.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Artists/{}/Images/Primary/-1", encoded(&image_name)),
                Credential::None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Artists/{}/Images/not-an-image/0", encoded(&image_name)),
                Credential::None,
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Artists/{}/Images/Primary/0", encoded("missing artist")),
                Credential::None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let _ = std::fs::remove_file(path);
    fixture.cleanup().await;
}

fn assert_artists(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: usize,
) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("artist items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("artist name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "MusicArtist"));
    assert!(items.iter().all(|item| item["IsFolder"] == true));
    assert!(body.get("items").is_none());
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

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    audio_id: Uuid,
    parent_id: Uuid,
    user_token: String,
    admin_token: String,
    alpha_artist_id: Uuid,
    alpha_artist: String,
    beta_artist: String,
    gamma_artist: String,
    movie_artist: String,
    album_artist: String,
    second_album_artist: String,
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

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("artist-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("artist-user-{suffix}"))
            .await
            .unwrap();
        let other_user = users
            .create(&format!("artist-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let audio = create_item(&items, "Audio", "Artist Track", None, false, "Audio").await;
        let video = create_item(&items, "MusicVideo", "Artist Video", None, false, "Video").await;
        let movie = create_item(&items, "Movie", "Movie Artist", None, false, "Video").await;
        let parent = create_item(&items, "Folder", "Artist Parent", None, true, "Folder").await;
        let nested_audio = create_item(
            &items,
            "Audio",
            "Nested Artist Track",
            Some(parent.id),
            false,
            "Audio",
        )
        .await;

        let values = ItemValueRepository::new(database.clone());
        let alpha_artist = format!("Alpha {suffix}");
        let beta_artist = format!("Beta {suffix}");
        let gamma_artist = format!("Gamma {suffix}");
        let movie_artist = format!("Movie {suffix}");
        let album_artist = format!("Album {suffix}");
        let second_album_artist = format!("Second Album {suffix}");
        let alpha = values
            .link(audio.id, item_value::ItemValueType::Artist, &alpha_artist)
            .await
            .expect("alpha artist");
        values
            .link(video.id, item_value::ItemValueType::Artist, &beta_artist)
            .await
            .expect("beta artist");
        values
            .link(
                nested_audio.id,
                item_value::ItemValueType::Artist,
                &gamma_artist,
            )
            .await
            .expect("gamma artist");
        values
            .link(movie.id, item_value::ItemValueType::Artist, &movie_artist)
            .await
            .expect("movie artist");
        values
            .link(audio.id, item_value::ItemValueType::Artist, &alpha_artist)
            .await
            .expect("duplicate artist link");
        values
            .link(
                audio.id,
                item_value::ItemValueType::AlbumArtist,
                &album_artist,
            )
            .await
            .expect("album artist");
        values
            .link(
                nested_audio.id,
                item_value::ItemValueType::AlbumArtist,
                &second_album_artist,
            )
            .await
            .expect("second album artist");

        let artist_item = create_item(&items, "MusicArtist", &beta_artist, None, true, "").await;
        let user_data = UserDataRepository::new(database.clone());
        let mut linked_item_favorite = NewUserData::new(audio.id, user.id, "LinkedArtistFavorite");
        linked_item_favorite.is_favorite = true;
        user_data
            .upsert(linked_item_favorite)
            .await
            .expect("linked item favorite user data");
        let mut artist_favorite = NewUserData::new(artist_item.id, user.id, "ArtistFavorite");
        artist_favorite.is_favorite = true;
        user_data
            .upsert(artist_favorite)
            .await
            .expect("artist favorite user data");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Artist Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            audio_id: audio.id,
            parent_id: parent.id,
            user_token,
            admin_token,
            alpha_artist_id: alpha.item_value_id,
            alpha_artist,
            beta_artist,
            gamma_artist,
            movie_artist,
            album_artist,
            second_album_artist,
        }
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

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    media_type: &str,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item.media_type = Some(media_type.to_owned());
    repository.create(item).await.expect("base item creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Artist Tests",
            "1.0",
            "Test",
            format!("artist-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn artist_route(name: &str) -> String {
    format!("/Artists/{}", encoded(name))
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
