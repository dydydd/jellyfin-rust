use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_music_scan_";

#[tokio::test]
async fn music_scan_builds_artist_album_audio_hierarchy() {
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
        exercise_music_scan(&task_database_name).await;
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

async fn exercise_music_scan(database_name: &str) {
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

    let library_root =
        std::env::temp_dir().join(format!("jellyfin-music-scan-{}", Uuid::new_v4().simple()));
    let artist = library_root.join("Artist");
    let album = artist.join("Album");
    std::fs::create_dir_all(&album).expect("music fixture directories");
    std::fs::write(album.join("track.flac"), b"not a real flac").expect("music fixture file");

    let folders = VirtualFolderService::new(database.clone());
    folders
        .create(
            "Music",
            Some("music".to_owned()),
            serde_json::json!({ "Enabled": true }),
            vec![library_root.to_string_lossy().into_owned()],
            false,
        )
        .await
        .expect("music virtual folder");

    let scan = LibraryScanService::with_probe_path(database.clone(), "missing-ffprobe");
    let summary = scan.scan_all().await.expect("music library scan");
    assert_eq!(summary.folders_seen, 1);

    let items = BaseItemRepository::new(database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let collection = items
        .children(root.id)
        .await
        .expect("collection children")
        .into_iter()
        .find(|item| item.item_type == "CollectionFolder")
        .expect("collection folder");
    let artist_item = items
        .children(collection.id)
        .await
        .expect("artist children")
        .into_iter()
        .find(|item| item.item_type == "MusicArtist")
        .expect("music artist");
    assert_eq!(
        artist_item.name.as_deref(),
        Some("Artist"),
        "artist folder name"
    );

    let album_item = items
        .children(artist_item.id)
        .await
        .expect("album children")
        .into_iter()
        .find(|item| item.item_type == "MusicAlbum")
        .expect("music album");
    assert_eq!(
        album_item.name.as_deref(),
        Some("Album"),
        "album folder name"
    );

    let audio = items
        .children(album_item.id)
        .await
        .expect("audio children")
        .into_iter()
        .find(|item| item.item_type == "Audio")
        .expect("audio track");
    assert_eq!(
        audio.parent_id,
        Some(album_item.id),
        "audio track parent must be the album"
    );
    assert!(
        audio
            .path
            .as_deref()
            .is_some_and(|path| path.ends_with("track.flac"))
    );

    std::fs::remove_dir_all(library_root).expect("music fixture cleanup");
    database.close().await.expect("database pool cleanup");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
