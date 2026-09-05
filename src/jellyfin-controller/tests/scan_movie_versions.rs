use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_movie_versions_";

#[tokio::test]
async fn movie_scan_groups_same_directory_metadata_without_filename_prefixes() {
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
        "jellyfin-movie-versions-{}",
        Uuid::new_v4().simple()
    ));
    let movie_directory = library_root.join("Feature Film");
    std::fs::create_dir_all(&movie_directory).expect("movie fixture directory");
    let paths = [
        movie_directory.join("Source.One.2160p.mkv"),
        movie_directory.join("Completely-Different-1080p.mkv"),
    ];
    for path in &paths {
        std::fs::write(path, b"not a real movie").expect("movie fixture write");
        std::fs::write(
            path.with_extension("nfo"),
            r#"<movie>
  <title>Feature Film</title>
  <year>2024</year>
  <uniqueid type="tmdb" default="true">101</uniqueid>
</movie>"#,
        )
        .expect("movie NFO write");
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
    scan.scan_all().await.expect("movie library scan");
    let repository = BaseItemRepository::new(database.clone());
    let path_strings = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let versions = repository
        .by_paths(&path_strings)
        .await
        .expect("movie version lookup");
    assert_eq!(versions.len(), 2);
    let primary = versions
        .iter()
        .find(|item| item.primary_version_id.is_none())
        .expect("one primary movie version");
    assert!(
        versions
            .iter()
            .filter(|item| item.id != primary.id)
            .all(|item| item.primary_version_id == Some(primary.id))
    );

    let before_repeat = repository
        .media_source_versions(primary.id)
        .await
        .expect("versions before repeat scan");
    scan.scan_all().await.expect("repeat movie library scan");
    let after_repeat = repository
        .media_source_versions(primary.id)
        .await
        .expect("versions after repeat scan");
    assert_eq!(
        after_repeat, before_repeat,
        "repeat scans must not rewrite an established version group"
    );

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
