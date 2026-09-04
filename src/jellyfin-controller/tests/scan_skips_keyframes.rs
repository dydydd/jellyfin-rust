use std::{os::unix::fs::PermissionsExt, path::Path};

use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{BaseItemRepository, DatabaseConfig, KeyframeDataRepository};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_no_keyframes_";

#[tokio::test]
async fn ordinary_library_scan_does_not_extract_keyframes() {
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
        std::env::temp_dir().join(format!("jellyfin-no-keyframes-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&library_root).expect("movie fixture directory");
    let movie_path = library_root.join("Movie.mkv");
    std::fs::write(&movie_path, b"not a real movie").expect("movie fixture write");

    let marker_path = library_root.join("keyframe-probe-invoked");
    let probe_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../jellyfin-media-encoding/tests/fixtures/probing/video_metadata.json");
    let probe_script = library_root.join("fake-ffprobe");
    std::fs::write(
        &probe_script,
        format!(
            "#!/bin/sh\nfor argument in \"$@\"; do\n  if [ \"$argument\" = \"-skip_frame\" ]; then\n    touch '{}'\n  fi\ndone\nexec /bin/cat '{}'\n",
            marker_path.display(),
            probe_fixture.display(),
        ),
    )
    .expect("fake ffprobe script");
    let mut permissions = std::fs::metadata(&probe_script)
        .expect("fake ffprobe metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&probe_script, permissions).expect("fake ffprobe executable");

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

    let scan = LibraryScanService::with_probe_path(database.clone(), &probe_script);
    scan.scan_all().await.expect("movie library scan");

    let movie = BaseItemRepository::new(database.clone())
        .get_by_type_and_name("Movie", "Movie")
        .await
        .expect("movie lookup")
        .expect("scanned movie");
    assert!(
        KeyframeDataRepository::new(database.clone())
            .get(movie.id)
            .await
            .expect("keyframe lookup")
            .is_none(),
        "ordinary scans must leave keyframe extraction to its dedicated workflow"
    );
    assert!(
        !marker_path.exists(),
        "ordinary scans must not invoke ffprobe's keyframe mode"
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
