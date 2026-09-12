use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_dot_ignore_";

#[tokio::test]
async fn movie_scan_applies_new_dot_ignore_rules_and_removes_stale_items() {
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
        "jellyfin-dot-ignore-scan-{}",
        Uuid::new_v4().simple()
    ));
    let ignored_directory = library_root.join("ignored");
    let visible_directory = library_root.join("visible");
    std::fs::create_dir_all(&ignored_directory).expect("ignored fixture directory");
    std::fs::create_dir_all(&visible_directory).expect("visible fixture directory");

    let kept_path = library_root.join("Always.mkv");
    let ignored_file_path = library_root.join("Skip.mkv");
    let ignored_child_path = ignored_directory.join("Hidden.mkv");
    let visible_child_path = visible_directory.join("Visible.mkv");
    for path in [
        &kept_path,
        &ignored_file_path,
        &ignored_child_path,
        &visible_child_path,
    ] {
        std::fs::write(path, b"video").expect("video fixture");
    }

    VirtualFolderService::new(database.clone())
        .create(
            "Movies",
            Some("movies".to_owned()),
            serde_json::json!({ "Enabled": true }),
            vec![library_root.to_string_lossy().into_owned()],
            false,
        )
        .await
        .expect("movie virtual folder");

    let scan = LibraryScanService::with_probe_path(database.clone(), "missing-ffprobe");
    let initial = scan.scan_all().await.expect("initial movie library scan");
    assert_eq!(initial.items_seen, 4);

    let paths = [
        &kept_path,
        &ignored_file_path,
        &ignored_child_path,
        &visible_child_path,
    ]
    .map(|path| path.to_string_lossy().into_owned());
    let repository = BaseItemRepository::new(database.clone());
    assert_eq!(
        repository
            .by_paths(&paths)
            .await
            .expect("initial scanned path lookup")
            .len(),
        4
    );

    // Official DotIgnoreIgnoreRule applies the nearest .ignore to both files
    // and directories. Library validation clears its directory lookup cache,
    // so adding this file after the first scan must take effect immediately.
    std::fs::write(library_root.join(".ignore"), "Skip.mkv\nignored/\n")
        .expect("dot-ignore fixture");

    let filtered = scan.scan_all().await.expect("filtered movie library scan");
    assert_eq!(filtered.items_seen, 2);
    assert_eq!(filtered.items_removed, 2);

    let remaining = repository
        .by_paths(&paths)
        .await
        .expect("filtered scanned path lookup");
    let remaining_paths = remaining
        .iter()
        .filter_map(|item| item.path.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(remaining_paths.len(), 2);
    assert!(remaining_paths.contains(&kept_path.to_str().expect("UTF-8 fixture path")));
    assert!(remaining_paths.contains(&visible_child_path.to_str().expect("UTF-8 fixture path")));
    assert!(!remaining_paths.contains(&ignored_file_path.to_str().expect("UTF-8 fixture path")));
    assert!(!remaining_paths.contains(&ignored_child_path.to_str().expect("UTF-8 fixture path")));

    std::fs::remove_dir_all(library_root).expect("movie fixture cleanup");
    database.close().await.expect("database pool cleanup");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
