use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_episode_versions_";

#[tokio::test]
async fn tv_scan_groups_episode_versions_and_preserves_the_group() {
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
    let outcome = tokio::spawn(async move { exercise_scan(&task_database_name).await }).await;

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

async fn exercise_scan(database_name: &str) {
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

    let library_root = std::env::temp_dir().join(format!(
        "jellyfin-episode-versions-{}",
        Uuid::new_v4().simple()
    ));
    let season_directory = library_root.join("Example Show").join("Season 01");
    std::fs::create_dir_all(&season_directory).expect("season fixture directory");
    let paths = [
        season_directory.join("Example Show - S01E01 - 2160p.mkv"),
        season_directory.join("Example Show - S01E01 - 1080p.mkv"),
    ];
    for path in &paths {
        std::fs::write(path, b"not a real episode").expect("episode fixture write");
    }

    VirtualFolderService::new(database.clone())
        .create(
            "TV Shows",
            Some("tvshows".to_owned()),
            serde_json::json!({ "Enabled": true }),
            vec![library_root.to_string_lossy().into_owned()],
            false,
        )
        .await
        .expect("tv virtual folder");

    let scan = LibraryScanService::with_probe_path(database.clone(), "missing-ffprobe");
    scan.scan_all().await.expect("tv library scan");
    let repository = BaseItemRepository::new(database.clone());
    let path_strings = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let versions = repository
        .by_paths(&path_strings)
        .await
        .expect("episode version lookup");
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().all(|item| item.item_type == "Episode"));
    let primary = versions
        .iter()
        .find(|item| item.primary_version_id.is_none())
        .expect("one primary episode version");
    assert!(
        versions
            .iter()
            .filter(|item| item.id != primary.id)
            .all(|item| item.primary_version_id == Some(primary.id))
    );
    assert!(
        versions
            .iter()
            .all(|item| item.series_id == primary.series_id && item.season_id == primary.season_id)
    );

    let before_repeat = repository
        .media_source_versions(primary.id)
        .await
        .expect("versions before repeat scan");
    scan.scan_all().await.expect("repeat tv library scan");
    let after_repeat = repository
        .media_source_versions(primary.id)
        .await
        .expect("versions after repeat scan");
    assert_eq!(
        after_repeat, before_repeat,
        "repeat scans must not rewrite an established episode version group"
    );

    std::fs::remove_dir_all(library_root).expect("episode fixture cleanup");
    database.close().await.expect("database pool cleanup");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
