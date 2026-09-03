use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_tv_structure_";

#[tokio::test]
async fn tv_scan_resolves_series_season_and_orphan_cleanup() {
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
        exercise_scan(&task_database_name).await;
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

#[allow(clippy::too_many_lines)]
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

    let library_root =
        std::env::temp_dir().join(format!("jellyfin-tv-structure-{}", Uuid::new_v4().simple()));
    let season_one = library_root.join("First Show").join("Season 01");
    std::fs::create_dir_all(&season_one).expect("season fixture directory");
    std::fs::create_dir_all(library_root.join("Flat Show")).expect("flat series fixture directory");
    std::fs::write(season_one.join("First Show S01E01.mkv"), b"episode one")
        .expect("season episode");
    std::fs::write(
        library_root.join("Flat Show").join("Flat Show S02E03.mkv"),
        b"episode",
    )
    .expect("flat episode");
    assert!(
        library_root
            .join("Flat Show")
            .join("Flat Show S02E03.mkv")
            .exists()
    );

    let folders = VirtualFolderService::new(database.clone());
    folders
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
    let summary = scan.scan_all().await.expect("tv library scan");
    let items = BaseItemRepository::new(database.clone());
    let first_series = items
        .get_by_type_and_name("Series", "First Show")
        .await
        .expect("series lookup")
        .unwrap_or_else(|| panic!("resolved series: {summary:?}"));
    assert_eq!(
        first_series.path.as_deref(),
        Some(library_root.join("First Show").to_str().unwrap())
    );
    let seasons = items
        .children(first_series.id)
        .await
        .expect("season children")
        .into_iter()
        .filter(|item| item.item_type == "Season")
        .collect::<Vec<_>>();
    let season = if seasons.len() == 1 {
        &seasons[0]
    } else {
        panic!("expected one resolved season, found {seasons:?}");
    };
    assert_eq!(season.index_number, Some(1));
    let episodes = items
        .children(season.id)
        .await
        .expect("episode children")
        .into_iter()
        .filter(|item| item.item_type == "Episode" || item.item_type == "Video")
        .collect::<Vec<_>>();
    let episode = if episodes.len() == 1 {
        &episodes[0]
    } else {
        panic!("expected one resolved episode, found {episodes:?}");
    };
    assert_eq!(episode.series_id, Some(first_series.id));
    assert_eq!(episode.season_id, Some(season.id));

    let flat_series = items
        .get_by_type_and_name("Series", "Flat Show")
        .await
        .expect("flat series lookup")
        .expect("resolved flat series");
    let flat_episode = items
        .descendants(flat_series.id)
        .await
        .expect("flat episode descendants")
        .into_iter()
        .find(|entry| entry.item.item_type == "Episode")
        .map(|entry| entry.item)
        .expect("resolved flat episode");
    let flat_season = items
        .descendants(flat_series.id)
        .await
        .expect("flat season descendants")
        .into_iter()
        .find(|entry| entry.item.item_type == "Season")
        .map(|entry| entry.item)
        .expect("resolved flat season");
    assert_eq!(flat_episode.parent_index_number, Some(2));
    assert_eq!(flat_episode.season_id, Some(flat_season.id));

    std::fs::remove_dir_all(&season_one).expect("season fixture removal");
    scan.scan_all().await.expect("incremental cleanup scan");
    let remaining = items
        .descendants(flat_series.id)
        .await
        .expect("remaining descendants");
    assert_eq!(remaining.len(), 2);
    assert!(
        items
            .get(season.id)
            .await
            .expect("season after cleanup")
            .is_none()
    );
    assert!(
        items
            .get(episode.id)
            .await
            .expect("episode after cleanup")
            .is_none()
    );
    assert!(
        items
            .get(first_series.id)
            .await
            .expect("series after cleanup")
            .is_some(),
        "series with no remaining media is retained"
    );

    std::fs::remove_dir_all(library_root).expect("tv fixture cleanup");
    database.close().await.expect("database pool cleanup");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
