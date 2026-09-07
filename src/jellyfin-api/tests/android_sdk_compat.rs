#![allow(clippy::too_many_lines)]

//! Android (jellyfin-sdk-kotlin) wire-compatibility harness.
//!
//! The Android client decodes every JSON body with kotlinx.serialization using
//! `isLenient = false`, so a single missing required property, wrong JSON scalar
//! type, or unknown enum member makes the *whole* response undecodable. iOS's
//! Swift SDK tolerates some of those shapes, which is why a page can look fine on
//! iPhone and break on Android.
//!
//! This test seeds a representative library, requests every endpoint the Android
//! player and browse flow touches, and writes each body plus the Kotlin model it
//! must decode into as `$JELLYFIN_ANDROID_DUMP/manifest.json`. `tools/kotlin_validate.py`
//! replays the committed Kotlin SDK schema over that dump.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamService, PersonReconciliationService, UserService};
use jellyfin_data::{
    BaseItemRepository, ChapterRepository, DatabaseConfig, DeviceRepository, NewBaseItem,
    NewChapter, NewDevice, NewPerson, NewPersonCredit, NewTrickplayInfo, PersonRepository,
    TrickplayInfoRepository,
};
use jellyfin_model::{MediaStream, MediaStreamType, UserPolicy};
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use percent_encoding::utf8_percent_encode;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::atomic::AtomicBool};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"kotlin-sdk-compat\", DeviceId=\"kotlin-sdk-compat\", Device=\"Test\", Version=\"1.0\"";
const LOGIN_AUTHORIZATION: &str = "MediaBrowser Client=\"kotlin-sdk-compat\", DeviceId=\"kotlin-sdk-login-compat\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_android_sdk_compat_";

/// `(route, kotlin model, credential)` for the Android surface. `admin` marks the
/// few reads the official server restricts to administrators.
const READS: &[(&str, &str, &str)] = &[
    ("/System/Info", "SystemInfo", "user"),
    ("/System/Info/Public", "PublicSystemInfo", "none"),
    ("/System/Configuration", "ServerConfiguration", "admin"),
    ("/System/Info/Storage", "SystemStorageDto", "admin"),
    ("/Users", "List<UserDto>", "admin"),
    ("/Users/Me", "UserDto", "user"),
    ("/Users/{user}", "UserDto", "user"),
    ("/Users/{user}/Views", "BaseItemDtoQueryResult", "user"),
    ("/UserViews", "BaseItemDtoQueryResult", "user"),
    ("/Items/Root", "BaseItemDto", "user"),
    ("/Items/Counts", "ItemCounts", "user"),
    ("/Items/Filters", "QueryFiltersLegacy", "user"),
    ("/Items/Filters2", "QueryFilters", "user"),
    (
        "/Items?limit=5&fields=MediaSourceCount,Chapters,Settings,ExternalUrls,RemoteTrailers,Tags",
        "BaseItemDtoQueryResult",
        "user",
    ),
    ("/Items/{movie}", "BaseItemDto", "user"),
    (
        "/Items/{movie}?fields=MediaSourceCount,Chapters,Settings,ExternalUrls,RemoteTrailers,Tags,People",
        "BaseItemDto",
        "user",
    ),
    ("/Users/{user}/Items/{movie}", "BaseItemDto", "user"),
    ("/Items/{movie}/Similar", "BaseItemDtoQueryResult", "user"),
    (
        "/Items/{movie}/Collections",
        "BaseItemDtoQueryResult",
        "user",
    ),
    ("/Items/{movie}/Intros", "BaseItemDtoQueryResult", "user"),
    (
        "/Items/{movie}/SpecialFeatures",
        "List<BaseItemDto>",
        "user",
    ),
    ("/Items/{movie}/ThemeMedia", "AllThemeMediaResult", "user"),
    ("/Items/{movie}/ThemeSongs", "ThemeMediaResult", "user"),
    ("/Items/{movie}/ThemeVideos", "ThemeMediaResult", "user"),
    (
        "/Items/{movie}/PlaybackInfo",
        "PlaybackInfoResponse",
        "user",
    ),
    (
        "/Items/{episode}/SpecialFeatures",
        "List<BaseItemDto>",
        "user",
    ),
    ("/Items/{audio}", "BaseItemDto", "user"),
    ("/Items/{episode}", "BaseItemDto", "user"),
    (
        "/Items/{episode}/PlaybackInfo",
        "PlaybackInfoResponse",
        "user",
    ),
    ("/Shows/{series}/Seasons", "BaseItemDtoQueryResult", "user"),
    ("/Shows/{series}/Episodes", "BaseItemDtoQueryResult", "user"),
    ("/Shows/NextUp", "BaseItemDtoQueryResult", "user"),
    ("/Shows/Upcoming", "BaseItemDtoQueryResult", "user"),
    (
        "/Videos/{movie}/AdditionalParts",
        "BaseItemDtoQueryResult",
        "user",
    ),
    ("/Items/Latest", "List<BaseItemDto>", "user"),
    (
        "/Users/{user}/Items/Resume?fields=MediaSourceCount",
        "BaseItemDtoQueryResult",
        "user",
    ),
    ("/Items/Suggestions", "BaseItemDtoQueryResult", "user"),
    ("/Years", "BaseItemDtoQueryResult", "user"),
    ("/Years/2011", "BaseItemDto", "user"),
    ("/Genres", "BaseItemDtoQueryResult", "user"),
    ("/Genres/Action", "BaseItemDto", "user"),
    ("/MusicGenres", "BaseItemDtoQueryResult", "user"),
    ("/Studios", "BaseItemDtoQueryResult", "user"),
    ("/Studios/Pixar", "BaseItemDto", "user"),
    ("/Persons", "BaseItemDtoQueryResult", "user"),
    ("/Artists", "BaseItemDtoQueryResult", "user"),
    ("/Artists/Test%20Artist", "BaseItemDto", "user"),
    ("/Artists/AlbumArtists", "BaseItemDtoQueryResult", "user"),
    ("/Search/Hints?searchTerm=movie", "SearchHintResult", "user"),
    ("/Sessions", "List<SessionInfoDto>", "admin"),
    (
        "/UserItems/{movie}/UserData?userId={user}",
        "UserItemDataDto",
        "user",
    ),
    (
        "/DisplayPreferences/ui-user-{user}?client=androidtv-native",
        "DisplayPreferencesDto",
        "user",
    ),
    ("/Items/{movie}/RemoteImages", "RemoteImageResult", "user"),
    (
        "/MediaSegments/{movie}",
        "MediaSegmentDtoQueryResult",
        "user",
    ),
    ("/Playlists/{playlist}", "PlaylistDto", "user"),
    (
        "/Playlists/{playlist}/Items",
        "BaseItemDtoQueryResult",
        "user",
    ),
    ("/Library/MediaFolders", "BaseItemDtoQueryResult", "admin"),
    (
        "/Items/{audio}/PlaybackInfo",
        "PlaybackInfoResponse",
        "user",
    ),
    ("/Items/{audio}/Similar", "BaseItemDtoQueryResult", "user"),
    (
        "/Libraries/AvailableOptions",
        "LibraryOptionsResultDto",
        "admin",
    ),
    ("/Branding/Configuration", "BrandingOptionsDto", "none"),
    ("/Movies/{movie}/Similar", "BaseItemDtoQueryResult", "user"),
    (
        "/Albums/{audio}/InstantMix",
        "BaseItemDtoQueryResult",
        "user",
    ),
    (
        "/Items/{movie}/InstantMix",
        "BaseItemDtoQueryResult",
        "user",
    ),
    ("/Trailers", "BaseItemDtoQueryResult", "user"),
    ("/Devices?limit=5", "DeviceInfoDtoQueryResult", "admin"),
];

#[tokio::test]
async fn android_sdk_wire_shapes_decode_with_kotlin_serialization() {
    let administrator = connect_default_database().await;
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(database_name.starts_with(DATABASE_PREFIX));
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

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

async fn connect_default_database() -> DatabaseConnection {
    jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available")
}

async fn exercise(database_name: &str) {
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

    let storage_root = std::env::temp_dir().join(database_name);
    let fixture = Fixture::new(&database, &storage_root).await;
    let mut failures = Vec::new();
    let mut dumped = Vec::new();

    // Login is the first Android/iOS SDK DTO boundary. Keep it in this
    // harness rather than only seeding a token, so a missing required nested
    // SessionInfo or User collection cannot make a mobile client fail before
    // it reaches any browse route.
    let authentication = fixture
        .app
        .clone()
        .oneshot(
            Request::post("/Users/AuthenticateByName")
                // Use a distinct device id so the official token replacement
                // behavior does not invalidate the seeded browse session.
                .header(header::AUTHORIZATION, LOGIN_AUTHORIZATION)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "Username": "compat-user",
                        "Pw": "compat-password"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let authentication_status = authentication.status();
    let authentication = to_bytes(authentication.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        authentication_status,
        StatusCode::OK,
        "login response: {}",
        String::from_utf8_lossy(&authentication)
    );
    dumped.push((
        "AuthenticationResult".to_owned(),
        serde_json::from_slice(&authentication).expect("login response must be JSON"),
    ));

    for (route, model, who) in READS {
        let resolved = fixture.resolve(route);
        let token = match *who {
            "admin" => &fixture.admin_token,
            _ => &fixture.user_token,
        };
        let response = fixture.request(Method::GET, &resolved, token).await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        if status != StatusCode::OK {
            failures.push(format!("{route} -> HTTP {status}"));
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            failures.push(format!("{route} -> body is not JSON"));
            continue;
        };
        dumped.push(((*model).to_owned(), value));
    }

    // The canonical Person item is created by people reconciliation, so resolve
    // its exposed name from the /Persons page rather than hard-coding it.
    let persons = dumped
        .iter()
        .find(|(model, value)| {
            model == "BaseItemDtoQueryResult"
                && value["Items"]
                    .as_array()
                    .is_some_and(|items| items.first().is_some_and(|i| i["Type"] == "Person"))
        })
        .map(|(_, value)| value.clone());
    if let Some(persons) = persons {
        let name = persons["Items"][0]["Name"].as_str().unwrap().to_owned();
        let encoded = utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC).to_string();
        let response = fixture
            .request(
                Method::GET,
                &format!("/Persons/{encoded}"),
                &fixture.user_token,
            )
            .await;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        match serde_json::from_slice::<Value>(&body) {
            Ok(value) if status == StatusCode::OK => {
                dumped.push(("BaseItemDto".to_owned(), value));
            }
            _ => failures.push(format!("/Persons/{name} -> HTTP {status}")),
        }
    } else {
        failures.push("/Persons returned no canonical Person items".to_owned());
    }

    // PlaybackInfo with an Android DeviceProfile body, which is how both Android
    // clients actually request playback.
    let playback_route = fixture.resolve("/Items/{movie}/PlaybackInfo");
    let response = fixture
        .request_json(
            Method::POST,
            &playback_route,
            &fixture.user_token,
            Some(&android_device_profile()),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    dumped.push((
        "PlaybackInfoResponse".to_owned(),
        serde_json::from_slice(&body).expect("playback info POST must be JSON"),
    ));

    if let Some(dir) = std::env::var_os("JELLYFIN_ANDROID_DUMP") {
        write_dump(&PathBuf::from(dir), &dumped);
    }

    fixture.database.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&storage_root);
    assert!(
        failures.is_empty(),
        "Android-critical endpoints did not return 200:\n{}",
        failures.join("\n")
    );
}

struct Fixture {
    database: DatabaseConnection,
    app: Router,
    user_token: String,
    admin_token: String,
    user_id: Uuid,
    playlist_id: Uuid,
    person_name: String,
    movie_id: Uuid,
    episode_id: Uuid,
    series_id: Uuid,
    audio_id: Uuid,
}

impl Fixture {
    async fn new(database: &DatabaseConnection, storage_root: &PathBuf) -> Self {
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator("compat-admin")
            .await
            .unwrap();
        let user = users.create("compat-user").await.unwrap();
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.unwrap();
        let movies = create_item(&items, "CollectionFolder", Some(root.id), "Movies", None);
        let shows = create_item(&items, "CollectionFolder", Some(root.id), "Shows", None);
        let music = create_item(&items, "CollectionFolder", Some(root.id), "Music", None);
        let (movies, shows, music) = (movies.await, shows.await, music.await);

        let mut new_movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        new_movie.parent_id = Some(movies.id);
        new_movie.name = Some("Test Movie".to_owned());
        new_movie.path = Some(format!("{}/Test Movie.mkv", storage_root.display()));
        new_movie.media_type = Some("Video".to_owned());
        new_movie.runtime_ticks = Some(7_200_000_000);
        new_movie.data = Some(json!({
            "Overview": "A test movie.",
            "Tagline": "Tagline",
            "Studios": ["Pixar"],
            "GenreItems": [{"Name": "Action", "Id": Uuid::new_v4().simple().to_string()}],
            "ProviderIds": {"ImdbId": "tt0000001", "Tmdb": "1"},
            "TagItems": [{"Name": "tag", "Id": Uuid::new_v4().simple().to_string()}],
            "RemoteTrailers": [{"Name": "Trailer", "Url": "https://example.invalid/t"}],
            "ExternalUrls": [{"Name": "IMDb", "Url": "https://example.invalid/imdb"}],
            "ProductionYear": 2011,
            "OfficialRating": "PG",
            "CustomRating": "Custom",
            "CommunityRating": 7.5,
            "MediaSourceContainer": "mkv",
            "Video3DFormat": "mvc",
            "ExtraType": "behindthescenes",
            "AirDays": ["monday"],
            "EndDate": "2020-01-02",
            "LockedFields": ["Taglines"],
            "LockData": false
        }));
        let movie = items.create(new_movie).await.unwrap();

        let series = create_item(&items, "Series", Some(shows.id), "Test Series", Some("Tv")).await;
        let season = create_item(&items, "Season", Some(series.id), "Season 1", None).await;
        let mut new_episode = NewBaseItem::new(Uuid::new_v4(), "Episode");
        new_episode.parent_id = Some(season.id);
        new_episode.name = Some("Pilot".to_owned());
        new_episode.path = Some(format!("{}/s01e01.mkv", storage_root.display()));
        new_episode.media_type = Some("Video".to_owned());
        new_episode.runtime_ticks = Some(2_700_000_000);
        new_episode.index_number = Some(1);
        new_episode.parent_index_number = Some(1);
        new_episode.series_id = Some(series.id);
        new_episode.season_id = Some(season.id);
        new_episode.data = Some(json!({"SeriesName": "Test Series", "SeasonName": "Season 1"}));
        let episode = items.create(new_episode).await.unwrap();

        let artist = create_item(&items, "MusicArtist", Some(music.id), "Test Artist", None).await;
        let album = create_item(&items, "MusicAlbum", Some(artist.id), "Test Album", None).await;
        let mut new_audio = NewBaseItem::new(Uuid::new_v4(), "Audio");
        new_audio.parent_id = Some(album.id);
        new_audio.name = Some("Test Song".to_owned());
        new_audio.path = Some(format!("{}/song.flac", storage_root.display()));
        new_audio.media_type = Some("Audio".to_owned());
        new_audio.runtime_ticks = Some(180_000_000);
        new_audio.data = Some(json!({
            "Album": "Test Album",
            "Artists": ["Test Artist"],
            "AlbumArtist": "Test Artist",
            "IndexNumber": 1
        }));
        let audio = items.create(new_audio).await.unwrap();

        std::fs::create_dir_all(storage_root).ok();
        std::fs::write(
            storage_root.join("Test Movie.mkv"),
            b"\x1a\x45\xdf\xa3fake-matroska",
        )
        .unwrap();
        std::fs::write(
            storage_root.join("s01e01.mkv"),
            b"\x1a\x45\xdf\xa3fake-matroska",
        )
        .unwrap();
        std::fs::write(storage_root.join("song.flac"), b"fLaC\x00\x00\x00\x22").unwrap();

        let streams = MediaStreamService::new(database.clone());
        streams
            .save_media_streams(
                movie.id,
                vec![
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Video,
                        codec: Some("h264".to_owned()),
                        codec_tag: Some("V_MPEG4/ISO/AVC".to_owned()),
                        width: Some(1920),
                        height: Some(1080),
                        aspect_ratio: Some("1.77777".to_owned()),
                        average_frame_rate: Some(23.976),
                        real_frame_rate: Some(23.976),
                        bit_rate: Some(12_000_000),
                        pixel_format: Some("yuv420p".to_owned()),
                        is_default: true,
                        is_interlaced: false,
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("eng".to_owned()),
                        channels: Some(2),
                        sample_rate: Some(48_000),
                        bit_rate: Some(320_000),
                        is_default: true,
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("ass".to_owned()),
                        language: Some("chi".to_owned()),
                        is_external: false,
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .unwrap();
        streams
            .save_media_streams(
                episode.id,
                vec![MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("hevc".to_owned()),
                    width: Some(3840),
                    height: Some(2160),
                    is_default: true,
                    ..MediaStream::default()
                }],
            )
            .await
            .unwrap();
        streams
            .save_media_streams(
                audio.id,
                vec![MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("flac".to_owned()),
                    channels: Some(2),
                    bit_rate: Some(900_000),
                    is_default: true,
                    ..MediaStream::default()
                }],
            )
            .await
            .unwrap();

        ChapterRepository::new(database.clone())
            .replace(
                movie.id,
                vec![NewChapter {
                    index_number: 0,
                    start_position_ticks: 0,
                    end_position_ticks: 3_600_000_000,
                    name: Some("Chapter 1".to_owned()),
                }],
            )
            .await
            .unwrap();

        TrickplayInfoRepository::new(database.clone())
            .upsert(
                movie.id,
                NewTrickplayInfo {
                    width: 320,
                    height: 180,
                    tile_width: 10,
                    tile_height: 10,
                    thumbnail_count: 100,
                    interval: 10_000,
                    bandwidth: 22_000,
                },
            )
            .await
            .unwrap();

        let policy = UserPolicy {
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            enable_all_folders: true,
            ..UserPolicy::default()
        };
        users.update_policy(user.id, &policy).await.unwrap();
        users
            .set_password_hash(
                user.id,
                DefaultAuthenticationProvider::new().password_hash("compat-password"),
            )
            .await
            .unwrap();

        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "kotlin-sdk-compat",
                "1.0",
                "Test",
                "kotlin-sdk-compat",
            ))
            .await
            .unwrap()
            .access_token;
        let admin_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                admin.id,
                "kotlin-sdk-compat-admin",
                "1.0",
                "Test",
                "kotlin-sdk-compat-admin",
            ))
            .await
            .unwrap()
            .access_token;

        let person_name = "Tom Hanks".to_owned();
        PersonRepository::new(database.clone())
            .replace_credits(
                movie.id,
                vec![NewPersonCredit {
                    person: NewPerson::new(person_name.clone()),
                    person_type: "Actor".to_owned(),
                    role: "Woody".to_owned(),
                    sort_order: Some(0),
                    list_order: 0,
                }],
            )
            .await
            .unwrap();
        let reconciliation = PersonReconciliationService::new(database.clone());
        reconciliation.set_item_by_name_directories(
            storage_root.join("programdata"),
            storage_root.join("metadata"),
        );
        reconciliation
            .reconcile(&AtomicBool::new(false))
            .await
            .unwrap();

        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Compat Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                storage_root.join("programdata"),
                storage_root.join("web"),
                storage_root.join("image-cache"),
                storage_root.join("cache"),
                storage_root.join("metadata"),
            ),
        );
        let playlist_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&format!(
                        "/Playlists?userId={}&name=Test%20Playlist",
                        user.id.simple()
                    ))
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{user_token}\""),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            "Name": "Test Playlist",
                            "Ids": [audio.id.simple().to_string()],
                            "UserId": user.id.simple().to_string(),
                            "MediaType": "Audio",
                            "Users": [],
                            "IsPublic": true
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(playlist_response.status(), StatusCode::OK);
        let playlist_body = serde_json::from_slice::<Value>(
            &to_bytes(playlist_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let playlist_id =
            Uuid::parse_str(playlist_body["Id"].as_str().expect("playlist id")).unwrap();

        Self {
            database: database.clone(),
            app,
            user_token,
            admin_token,
            user_id: user.id,
            playlist_id,
            person_name: "Tom%20Hanks".to_owned(),
            movie_id: movie.id,
            episode_id: episode.id,
            series_id: series.id,
            audio_id: audio.id,
        }
    }

    fn resolve(&self, route: &str) -> String {
        route
            .replace("{user}", &self.user_id.simple().to_string())
            .replace("{movie}", &self.movie_id.simple().to_string())
            .replace("{episode}", &self.episode_id.simple().to_string())
            .replace("{series}", &self.series_id.simple().to_string())
            .replace("{audio}", &self.audio_id.simple().to_string())
            .replace("{playlist}", &self.playlist_id.simple().to_string())
            .replace("{person}", &self.person_name)
    }

    async fn request(&self, method: Method, uri: &str, token: &str) -> axum::response::Response {
        self.request_json(method, uri, token, None).await
    }

    async fn request_json(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<&Value>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri).header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(body).unwrap())
            }
            None => Body::empty(),
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    name: &str,
    collection_type: Option<&str>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = parent_id;
    item.name = Some(name.to_owned());
    item.is_folder = matches!(
        item_type,
        "CollectionFolder" | "Series" | "Season" | "MusicArtist" | "MusicAlbum"
    );
    if let Some(collection_type) = collection_type {
        item.data = Some(json!({ "CollectionType": collection_type }));
    }
    repository.create(item).await.unwrap()
}

fn android_device_profile() -> Value {
    json!({
        "MaxStreamingBitrate": 120_000_000,
        "MaxStaticBitrate": 120_000_000,
        "MusicStreamingTranscodingBitrate": 192_000,
        "DirectPlayProfiles": [
            {"Container": "mkv", "Type": "Video", "VideoCodec": "h264,hevc", "AudioCodec": "aac,ac3,eac3,flac"},
            {"Container": "mp4", "Type": "Video"},
            {"Container": "flac,mp3", "Type": "Audio"}
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac", "Protocol": "hls", "Context": "Streaming", "MaxAudioChannels": "6", "MinSegments": 1},
            {"Container": "mp3", "Type": "Audio", "AudioCodec": "mp3", "Protocol": "http", "Context": "Streaming", "MaxAudioChannels": "2"}
        ],
        "StreamContainerProfiles": [],
        "CodecProfiles": [
            {"Type": "Video", "Container": "mkv", "CodecTag": "V_MPEG4/ISO/AVC", "Conditions": [{"Condition": 0, "Property": "Width", "Value": "1920", "IsRequired": true}]}
        ],
        "SubtitleProfiles": [
            {"Format": "ass", "Method": "External"},
            {"Format": "srt", "Method": "External"},
            {"Format": "subrip", "Method": "Embed"},
            {"Format": "pgssub", "Method": "Encode"},
            {"Format": "vobsub", "Method": "NoEncode"}
        ],
        "ResponseProfiles": [],
        "ContainerProfiles": [],
        "ProfileConditions": [],
        "XmlRootAttributes": [],
        "EnableStreamCopyBuffers": false,
        "EnableDirectStreamCopies": false,
        "EnableHardwareEncoding": true,
        "AllowVideoStreamCopy": true,
        "EnableAudioVbrEncoding": true,
        "DeinterlaceVideo": true,
        "DetectInterlacedVideo": true,
        "EnableSegmentMaintenance": true,
        "EnableSegmentDeletion": false,
        "SegmentLength": 6,
        "Throttling": 0,
        "TargetTsSegmentSizeBytes": 0
    })
}

fn write_dump(dir: &PathBuf, dumped: &[(String, Value)]) {
    std::fs::create_dir_all(dir).unwrap();
    let mut manifest = Vec::new();
    for (index, (model, value)) in dumped.iter().enumerate() {
        let file = format!("{index:03}.json");
        std::fs::write(dir.join(&file), serde_json::to_vec_pretty(value).unwrap()).unwrap();
        manifest.push(json!({"file": file, "model": model}));
    }
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&Value::Array(manifest)).unwrap(),
    )
    .unwrap();
}
