use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig, MediaStreamQuery, MediaStreamRepository};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_strm_";

#[tokio::test]
async fn movie_scan_indexes_strm_pointer_and_target() {
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

    let library_root =
        std::env::temp_dir().join(format!("jellyfin-strm-scan-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&library_root).expect("movie fixture directory");
    let strm_path = library_root.join("Pointer Movie.strm");
    let target = "/CloudNAS/Movies/Pointer Movie.mkv";
    std::fs::write(&strm_path, format!("\n {target} \r\n")).expect("STRM fixture write");

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
    let summary = scan.scan_all().await.expect("STRM library scan");
    assert_eq!(summary.folders_seen, 1);

    let movie = BaseItemRepository::new(database.clone())
        .get_by_type_and_name("Movie", "Pointer Movie")
        .await
        .expect("movie lookup")
        .unwrap_or_else(|| panic!("scanned STRM movie item: {summary:?}"));
    assert_eq!(movie.path.as_deref(), strm_path.to_str());
    assert_eq!(movie.data.as_ref().unwrap()["StrmTarget"], target);
    assert_eq!(movie.data.as_ref().unwrap()["Container"], "mkv");

    let streams = MediaStreamRepository::new(database.clone())
        .query(MediaStreamQuery {
            item_id: movie.id,
            stream_index: None,
            stream_type: None,
        })
        .await
        .expect("media stream query");
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].stream_index, 0);

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
