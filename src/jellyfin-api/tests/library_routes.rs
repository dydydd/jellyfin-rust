#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository,
    LinkedChildRepository, NewBaseItem, NewDevice, NewUserData, UserDataRepository,
    entities::item_value,
    entities::{user, user_data},
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Library Tests\", DeviceId=\"library-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_library_routes_";
static LIBRARY_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn official_library_controller_missing_item_contract() {
    let _guard = LIBRARY_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let missing = Uuid::new_v4();
    for route in [
        format!("/Items/{missing}/File"),
        format!("/Items/{missing}/ThemeSongs"),
        format!("/Items/{missing}/ThemeVideos"),
        format!("/Items/{missing}/ThemeMedia"),
        format!("/Items/{missing}/Ancestors"),
        format!("/Items/{missing}/Download"),
        format!("/Items/{missing}/Collections"),
        format!("/Artists/{missing}/Similar"),
        format!("/Items/{missing}/Similar"),
        format!("/Albums/{missing}/Similar"),
        format!("/Shows/{missing}/Similar"),
        format!("/Movies/{missing}/Similar"),
        format!("/Trailers/{missing}/Similar"),
        format!("/Items/{missing}/InstantMix"),
    ] {
        assert_eq!(
            fixture
                .request("GET", &route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{route}"
        );
    }
    for route in [format!("/Items/{missing}"), format!("/Items?ids={missing}")] {
        assert_eq!(
            fixture.request("DELETE", &route, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "{route}"
        );
        assert_eq!(
            fixture
                .request("DELETE", &route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{route}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn ancestors_download_similar_and_empty_relationships_have_real_success_semantics() {
    let _guard = LIBRARY_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    assert_ancestors(&fixture).await;
    assert_streamed_downloads(&fixture).await;
    assert_similar_items(&fixture).await;
    assert_relationships(&fixture).await;
    assert_item_counts(&fixture).await;
    assert_instant_mix(&fixture).await;
    assert_audio_stream(&fixture).await;
    assert_video_stream(&fixture).await;
    fixture.cleanup().await;
}

async fn assert_video_stream(fixture: &Fixture) {
    let media_bytes = Fixture::media_bytes();
    let route = format!("/Videos/{}/stream.mkv", fixture.child_id);
    let response = fixture
        .request("GET", &route, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        media_bytes
    );
    let head = fixture
        .request("HEAD", &route, Some(&fixture.user_token))
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(
        to_bytes(head.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    let range = fixture.range_request(&route, "bytes=30-39").await;
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        to_bytes(range.into_body(), usize::MAX).await.unwrap(),
        &media_bytes[30..=39]
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Videos/{}/stream.mp4", fixture.child_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Videos/{}/stream?static=false", fixture.child_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let strm_route = format!("/Videos/{}/stream.mkv", fixture.strm_id);
    let response = fixture
        .request("GET", &strm_route, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        media_bytes
    );
}

async fn assert_audio_stream(fixture: &Fixture) {
    let media_bytes = Fixture::media_bytes();
    let route = format!("/Audio/{}/stream.bin", fixture.stream_audio_id);
    assert_eq!(
        fixture.request("GET", &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let response = fixture
        .request("GET", &route, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        media_bytes.len().to_string()
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        media_bytes
    );
    let head = fixture
        .request("HEAD", &route, Some(&fixture.user_token))
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH],
        Fixture::MEDIA_SIZE.to_string()
    );
    assert!(
        to_bytes(head.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );

    let range = fixture.range_request(&route, "bytes=10-19").await;
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        to_bytes(range.into_body(), usize::MAX).await.unwrap(),
        &media_bytes[10..=19]
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Audio/{}/stream.mp3", fixture.stream_audio_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Audio/{}/stream?static=false", fixture.stream_audio_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let universal = format!(
        "/Audio/{}/universal?container=mp3,bin|pcm",
        fixture.stream_audio_id
    );
    let response = fixture
        .request("GET", &universal, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        media_bytes
    );
    let universal_range = fixture.range_request(&universal, "bytes=20-29").await;
    assert_eq!(universal_range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        to_bytes(universal_range.into_body(), usize::MAX)
            .await
            .unwrap(),
        &media_bytes[20..=29]
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!(
                    "/Audio/{}/universal?container=mp3&transcodingContainer=mp3",
                    fixture.stream_audio_id
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

async fn assert_instant_mix(fixture: &Fixture) {
    let song_route = format!("/Songs/{}/InstantMix?limit=2", fixture.song_id);
    let mix = fixture.json("GET", &song_route, &fixture.user_token).await;
    assert_eq!(mix["TotalRecordCount"], 3);
    assert_eq!(mix["Items"].as_array().unwrap().len(), 2);
    assert_eq!(mix["Items"][0]["Id"], fixture.song_id.simple().to_string());
    assert!(
        mix["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Audio")
    );
    assert!(
        mix["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Id"] != fixture.other_genre_song_id.simple().to_string())
    );

    let non_audio_route = format!("/Items/{}/InstantMix", fixture.album_id);
    let album_mix = fixture
        .json("GET", &non_audio_route, &fixture.user_token)
        .await;
    assert_eq!(album_mix["TotalRecordCount"], 3);
    assert!(
        album_mix["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Audio")
    );

    let genre_route = format!(
        "/MusicGenres/InstantMix?id={}&limit=1",
        fixture.instant_genre_id
    );
    let genre_mix = fixture.json("GET", &genre_route, &fixture.user_token).await;
    assert_eq!(genre_mix["TotalRecordCount"], 3);
    assert_eq!(genre_mix["Items"].as_array().unwrap().len(), 1);
    assert_eq!(genre_mix["Items"][0]["Type"], "Audio");

    let genre_name_mix = fixture
        .json(
            "GET",
            "/MusicGenres/Post%20Rock/InstantMix?limit=1",
            &fixture.user_token,
        )
        .await;
    assert_eq!(genre_name_mix["TotalRecordCount"], 3);
    assert_eq!(genre_name_mix["Items"].as_array().unwrap().len(), 1);

    let legacy_artist_route = format!("/Artists/InstantMix?id={}&limit=1", fixture.album_id);
    let legacy_mix = fixture
        .json("GET", &legacy_artist_route, &fixture.user_token)
        .await;
    assert_eq!(legacy_mix["TotalRecordCount"], 3);
    assert_eq!(legacy_mix["Items"].as_array().unwrap().len(), 1);
}

async fn assert_ancestors(fixture: &Fixture) {
    let ancestors = fixture
        .json(
            "GET",
            &format!("/Items/{}/Ancestors", fixture.child_id),
            &fixture.user_token,
        )
        .await;
    let ancestor_ids = ancestors
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ancestor_ids,
        vec![
            fixture.parent_id.simple().to_string(),
            jellyfin_data::USER_ROOT_FOLDER_ID.simple().to_string()
        ]
    );
}

async fn assert_streamed_downloads(fixture: &Fixture) {
    let media_bytes = Fixture::media_bytes();
    for (route, attachment) in [
        (format!("/Items/{}/File", fixture.child_id), false),
        (format!("/Items/{}/Download", fixture.child_id), true),
    ] {
        let response = fixture
            .request("GET", &route, Some(&fixture.user_token))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            media_bytes.len().to_string()
        );
        assert_eq!(
            response.headers().contains_key(header::CONTENT_DISPOSITION),
            attachment
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), media_bytes);
    }
    let response = fixture
        .request(
            "GET",
            &format!("/Items/{}/Download", fixture.strm_id),
            Some(&fixture.user_token),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        media_bytes
    );
    let range_start = 65_530;
    let range_end = 65_550;
    let response = fixture
        .range_request(
            &format!("/Items/{}/Download", fixture.child_id),
            &format!("bytes={range_start}-{range_end}"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()[header::CONTENT_RANGE],
        format!("bytes {range_start}-{range_end}/{}", media_bytes.len())
    );
    let range_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(range_body.as_ref(), &media_bytes[range_start..=range_end]);
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Items/{}/Download", fixture.missing_file_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Items/{}/Download", fixture.io_error_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

async fn assert_similar_items(fixture: &Fixture) {
    let similar = fixture
        .json(
            "GET",
            &format!("/Movies/{}/Similar", fixture.child_id),
            &fixture.user_token,
        )
        .await;
    let similar_items = similar["Items"].as_array().unwrap();
    assert!(similar["TotalRecordCount"].as_u64().unwrap() >= 1);
    assert!(
        similar_items
            .iter()
            .any(|item| item["Id"] == fixture.similar_id.simple().to_string())
    );
    assert!(
        similar_items
            .iter()
            .all(|item| item["Id"] != fixture.child_id.simple().to_string())
    );
    assert!(similar_items.iter().all(|item| item["Type"] == "Movie"));
    assert!(
        similar_items
            .iter()
            .all(|item| item.get("item_type").is_none())
    );
}

async fn assert_relationships(fixture: &Fixture) {
    for route in [
        format!("/Items/{}/ThemeSongs", fixture.child_id),
        format!("/Items/{}/ThemeVideos", fixture.child_id),
    ] {
        let body = fixture.json("GET", &route, &fixture.user_token).await;
        assert_eq!(body["OwnerId"], fixture.child_id.to_string());
        assert_eq!(body["TotalRecordCount"], 0);
        assert!(body["Items"].as_array().unwrap().is_empty());
    }
    let collections = fixture
        .json(
            "GET",
            &format!("/Items/{}/Collections", fixture.child_id),
            &fixture.user_token,
        )
        .await;
    let all_collection_items = collections["Items"].as_array().unwrap();
    assert_eq!(collections["TotalRecordCount"], 2);
    assert_eq!(all_collection_items.len(), 2);
    assert_eq!(
        all_collection_items
            .iter()
            .map(|item| item["Id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        [
            fixture.first_collection_id.simple().to_string(),
            fixture.second_collection_id.simple().to_string()
        ],
    );

    let collections = fixture
        .json(
            "GET",
            &format!(
                "/Items/{}/Collections?startIndex=1&limit=1",
                fixture.child_id
            ),
            &fixture.user_token,
        )
        .await;
    assert_eq!(collections["TotalRecordCount"], 2);
    assert_eq!(collections["StartIndex"], 1);
    let collection_items = collections["Items"].as_array().unwrap();
    assert_eq!(collection_items.len(), 1);
    assert_eq!(
        collection_items[0]["Id"],
        fixture.second_collection_id.simple().to_string()
    );
    assert_eq!(collection_items[0]["Type"], "BoxSet");
}

async fn assert_item_counts(fixture: &Fixture) {
    assert_eq!(
        fixture.request("GET", "/Items/Counts", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "GET",
                &format!("/Items/Counts?userId={}&isFavorite=true", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let counts = fixture
        .json("GET", "/Items/Counts?isFavorite=true", &fixture.user_token)
        .await;
    assert_eq!(
        counts,
        json!({
            "MovieCount": 1,
            "SeriesCount": 1,
            "EpisodeCount": 1,
            "ArtistCount": 1,
            "ProgramCount": 1,
            "TrailerCount": 1,
            "SongCount": 1,
            "AlbumCount": 1,
            "MusicVideoCount": 1,
            "BoxSetCount": 1,
            "BookCount": 1,
            "ItemCount": 11
        })
    );

    let administrator_counts = fixture
        .json(
            "GET",
            &format!("/Items/Counts?userId={}&isFavorite=true", fixture.user_id),
            &fixture.admin_token,
        )
        .await;
    assert_eq!(administrator_counts, counts);
}

#[tokio::test]
async fn administrator_single_and_batch_deletion_are_atomic_and_database_only() {
    let _guard = LIBRARY_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .request(
                "DELETE",
                &format!("/Items/{}", fixture.parent_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let missing = Uuid::new_v4();
    let atomic_route = format!("/Items?ids={},{}", fixture.parent_id, missing);
    assert_eq!(
        fixture
            .request("DELETE", &atomic_route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(fixture.items().exists(fixture.parent_id).await.unwrap());

    assert_eq!(
        fixture
            .request(
                "DELETE",
                &format!("/Items/{}", fixture.single_delete_id),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let batch_route = format!("/Items?ids={},{}", fixture.parent_id, fixture.similar_id);
    assert_eq!(
        fixture
            .request("DELETE", &batch_route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    for id in [
        fixture.parent_id,
        fixture.child_id,
        fixture.grandchild_id,
        fixture.similar_id,
        fixture.single_delete_id,
    ] {
        assert!(!fixture.items().exists(id).await.unwrap());
    }
    assert_eq!(
        user_data::Entity::find()
            .filter(user_data::Column::UserId.eq(fixture.user_id))
            .count(&fixture.database)
            .await
            .unwrap(),
        0
    );
    assert!(!tokio::fs::try_exists(&fixture.media_path).await.unwrap());
    fixture.cleanup().await;
}

struct Fixture {
    administrator: DatabaseConnection,
    database: DatabaseConnection,
    database_name: String,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    parent_id: Uuid,
    child_id: Uuid,
    grandchild_id: Uuid,
    similar_id: Uuid,
    song_id: Uuid,
    album_id: Uuid,
    genre_song_ids: Vec<Uuid>,
    other_genre_song_id: Uuid,
    instant_genre_id: Uuid,
    stream_audio_id: Uuid,
    strm_id: Uuid,
    first_collection_id: Uuid,
    second_collection_id: Uuid,
    single_delete_id: Uuid,
    missing_file_id: Uuid,
    io_error_id: Uuid,
    media_path: String,
    audio_path: String,
    strm_path: String,
}

impl Fixture {
    const MEDIA_SIZE: usize = 192 * 1024 + 17;

    fn media_bytes() -> Vec<u8> {
        (0..Self::MEDIA_SIZE)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect()
    }

    async fn new() -> Self {
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert_temporary_database_name(&database_name);
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 16,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database).await.expect("migrations");
        for pattern in ["library-admin-%", "library-user-%"] {
            user::Entity::delete_many()
                .filter(user::Column::Username.like(pattern))
                .exec(&database)
                .await
                .expect("stale library test users");
        }
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("library-admin-{suffix}"))
            .await
            .expect("library admin");
        let user = users
            .create(&format!("library-user-{suffix}"))
            .await
            .expect("library user");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("library-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("library-user-{suffix}")).await;

        let media_path = format!("/tmp/jellyfin-rust-library-{suffix}.mkv");
        tokio::fs::write(&media_path, Self::media_bytes())
            .await
            .expect("library media fixture");
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let parent = create_item(&items, "Folder", "Library Parent", root.id, None).await;
        let audio_path = format!("/tmp/jellyfin-rust-audio-{suffix}.bin");
        tokio::fs::write(&audio_path, Self::media_bytes())
            .await
            .expect("audio media fixture");
        let stream_audio =
            create_item(&items, "Audio", "Stream Audio", root.id, Some(&audio_path)).await;
        let child = create_item(
            &items,
            "Movie",
            "Library Movie",
            parent.id,
            Some(&media_path),
        )
        .await;
        let strm_path = format!("/tmp/jellyfin-rust-video-{suffix}.strm");
        tokio::fs::write(&strm_path, format!("{media_path}\n"))
            .await
            .expect("STRM media fixture");
        let mut strm = create_item(&items, "Movie", "STRM Movie", root.id, Some(&strm_path)).await;
        strm.data = Some(json!({
            "Container": "mkv",
            "StrmTarget": media_path
        }));
        let strm = items.update(strm).await.expect("STRM item metadata");
        let grandchild = create_item(&items, "Episode", "Library Episode", child.id, None).await;
        let mut count_items = Vec::new();
        for (item_type, name) in [
            ("Series", "Count Series"),
            ("MusicArtist", "Count Artist"),
            ("Program", "Count Program"),
            ("Trailer", "Count Trailer"),
            ("Audio", "Count Song"),
            ("MusicAlbum", "Count Album"),
            ("MusicVideo", "Count Music Video"),
            ("BoxSet", "Count Box Set"),
            ("Book", "Count Book"),
        ] {
            count_items.push(create_item(&items, item_type, name, parent.id, None).await);
        }
        let similar = create_item(&items, "Movie", "Similar Movie", root.id, None).await;
        let second_collection =
            create_item(&items, "BoxSet", "Zulu Collection", root.id, None).await;
        let first_collection =
            create_item(&items, "BoxSet", "Alpha Collection", root.id, None).await;
        LinkedChildRepository::new(database.clone())
            .add_manual(second_collection.id, &[child.id])
            .await
            .expect("second collection link");
        LinkedChildRepository::new(database.clone())
            .add_manual(first_collection.id, &[child.id])
            .await
            .expect("first collection link");
        let song = create_item(&items, "Audio", "Instant Seed", root.id, None).await;
        let song_two = create_item(&items, "Audio", "Instant Two", root.id, None).await;
        let song_three = create_item(&items, "Audio", "Instant Three", root.id, None).await;
        let other_genre_song = create_item(&items, "Audio", "Other Genre", root.id, None).await;
        let album = create_item(&items, "MusicAlbum", "Instant Album", root.id, None).await;
        let values = ItemValueRepository::new(database.clone());
        let mut instant_genre_id = Uuid::nil();
        for id in [song.id, song_two.id, song_three.id, album.id] {
            instant_genre_id = values
                .link(id, item_value::ItemValueType::Genre, "Post Rock")
                .await
                .unwrap()
                .item_value_id;
        }
        values
            .link(
                other_genre_song.id,
                item_value::ItemValueType::Genre,
                "Jazz",
            )
            .await
            .unwrap();
        let single_delete = create_item(&items, "Video", "Single Delete", root.id, None).await;
        let missing_file = create_item(
            &items,
            "Video",
            "Missing File",
            root.id,
            Some(&format!("/tmp/jellyfin-rust-missing-{suffix}.mkv")),
        )
        .await;
        let oversized_component = "x".repeat(5_000);
        let io_error = create_item(
            &items,
            "Video",
            "I/O Error",
            root.id,
            Some(&format!("/tmp/{oversized_component}")),
        )
        .await;
        let mut resume = NewUserData::new(grandchild.id, user.id, "library");
        resume.playback_position_ticks = 100;
        let user_data = UserDataRepository::new(database.clone());
        user_data.upsert(resume).await.expect("library user data");
        for item_id in std::iter::once(child.id)
            .chain(std::iter::once(grandchild.id))
            .chain(count_items.into_iter().map(|item| item.id))
        {
            favorite(&user_data, user.id, item_id).await;
        }
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Library Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            administrator,
            database,
            database_name,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            parent_id: parent.id,
            child_id: child.id,
            grandchild_id: grandchild.id,
            similar_id: similar.id,
            song_id: song.id,
            album_id: album.id,
            genre_song_ids: vec![song.id, song_two.id, song_three.id],
            other_genre_song_id: other_genre_song.id,
            instant_genre_id,
            stream_audio_id: stream_audio.id,
            strm_id: strm.id,
            first_collection_id: first_collection.id,
            second_collection_id: second_collection.id,
            single_delete_id: single_delete.id,
            missing_file_id: missing_file.id,
            io_error_id: io_error.id,
            media_path,
            audio_path,
            strm_path,
        }
    }

    fn items(&self) -> BaseItemRepository {
        BaseItemRepository::new(self.database.clone())
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
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

    async fn json(&self, method: &str, uri: &str, token: &str) -> Value {
        let response = self.request(method, uri, Some(token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn range_request(&self, uri: &str, range: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{}\"", self.user_token),
                    )
                    .header(header::RANGE, range)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        let items = self.items();
        for id in [
            self.parent_id,
            self.similar_id,
            self.album_id,
            self.other_genre_song_id,
            self.stream_audio_id,
            self.strm_id,
            self.single_delete_id,
            self.missing_file_id,
            self.io_error_id,
        ] {
            items.delete(id).await.expect("library item cleanup");
        }
        items
            .delete_many(&self.genre_song_ids)
            .await
            .expect("instant mix song cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("library users cleanup");
        let _ = tokio::fs::remove_file(self.media_path).await;
        let _ = tokio::fs::remove_file(self.audio_path).await;
        let _ = tokio::fs::remove_file(self.strm_path).await;
        drop(items);
        self.database.close().await.expect("database pool cleanup");
        self.administrator
            .execute_unprepared(&format!(
                "DROP DATABASE {} WITH (FORCE)",
                self.database_name
            ))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        self.administrator
            .close()
            .await
            .expect("administrator database pool cleanup");
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    path: Option<&str>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    item.path = path.map(ToOwned::to_owned);
    item.media_type = Some("Video".to_owned());
    item.is_folder = item_type == "Folder";
    repository.create(item).await.expect("library item")
}

async fn favorite(repository: &UserDataRepository, user_id: Uuid, item_id: Uuid) {
    let mut data = NewUserData::new(item_id, user_id, format!("library-favorite-{item_id}"));
    data.is_favorite = true;
    repository.upsert(data).await.expect("favorite user data");
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Library Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("library session")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
