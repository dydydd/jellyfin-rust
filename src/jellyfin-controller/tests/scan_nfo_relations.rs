use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, ItemValueRepository, PersonRepository,
    entities::item_value::ItemValueType,
};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_nfo_relations_";

#[tokio::test]
async fn movie_scan_links_nfo_genres_tags_studios_and_people() {
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
        "jellyfin-nfo-relations-{}",
        Uuid::new_v4().simple()
    ));
    let storage_root = std::env::temp_dir().join(format!(
        "jellyfin-nfo-relations-storage-{}",
        Uuid::new_v4().simple()
    ));
    let metadata_root = storage_root.join("metadata");
    std::fs::create_dir_all(&library_root).expect("movie fixture directory");
    std::fs::write(library_root.join("Scan Movie.mkv"), b"not a real movie").unwrap();
    std::fs::write(
        library_root.join("Scan Movie.nfo"),
        r#"<?xml version="1.0" encoding="utf-8"?>
<movie>
  <title>Scan Movie</title>
  <genre>Drama</genre>
  <tag>Favorite</tag>
  <style>Neo-Noir</style>
  <studio>Example Studio</studio>
  <actor>
    <name>Jane Actor</name>
    <role>Lead</role>
    <order>0</order>
  </actor>
  <director>John Director</director>
</movie>
"#,
    )
    .expect("movie NFO write");

    let folders = VirtualFolderService::new(database.clone());
    folders
        .create(
            "Movies",
            Some("movies".to_owned()),
            serde_json::json!({ "Enabled": true }),
            vec![library_root.to_string_lossy().into_owned()],
            false,
        )
        .await
        .expect("movie virtual folder");
    let collection_id = folders
        .list()
        .await
        .expect("virtual folder list")
        .into_iter()
        .next()
        .expect("movie virtual folder")
        .id;

    let scan = LibraryScanService::with_probe_path(database.clone(), "missing-ffprobe");
    scan.set_item_by_name_directories(&storage_root, &metadata_root);
    let summary = scan.scan_all().await.expect("movie library scan");
    assert_eq!(summary.folders_seen, 1);

    let items = BaseItemRepository::new(database.clone());
    let movie = items
        .get_by_type_and_name("Movie", "Scan Movie")
        .await
        .expect("movie lookup")
        .expect("scanned movie item");

    let values = ItemValueRepository::new(database.clone());
    let genres = values
        .values_for_item(movie.id, ItemValueType::Genre)
        .await
        .expect("genre query");
    assert!(
        genres.iter().any(|value| value.value == "Drama"),
        "NFO genre must be linked"
    );
    let genre = items
        .get_by_type_and_name("Genre", "Drama")
        .await
        .expect("genre entity lookup")
        .expect("full scan must reconcile the persisted Genre entity");
    assert!(genre.is_folder);
    assert_eq!(
        genre.presentation_unique_key.as_deref(),
        Some("Genre-Drama")
    );
    assert!(metadata_root.join("Genre").join("Drama").is_dir());

    let tags = values
        .values_for_item(movie.id, ItemValueType::Tags)
        .await
        .expect("tag query");
    assert_eq!(
        tags.iter()
            .map(|value| value.value.as_str())
            .collect::<Vec<_>>(),
        ["Favorite", "Neo-Noir"],
        "NFO tags must be linked"
    );
    assert_eq!(
        movie
            .data
            .as_ref()
            .and_then(|data| data.get("Tags"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>(),
        ["Favorite", "Neo-Noir"],
        "NFO tags must be persisted in item metadata"
    );
    let studios = values
        .values_for_item(movie.id, ItemValueType::Studios)
        .await
        .expect("studio query");
    assert!(
        studios.iter().any(|value| value.value == "Example Studio"),
        "NFO studio must be linked"
    );

    let people = PersonRepository::new(database.clone())
        .people_for_item(movie.id)
        .await
        .expect("people query");
    assert!(
        people
            .iter()
            .any(|credit| credit.person_type == "Actor" && credit.person.name == "Jane Actor"),
        "NFO actor must be linked"
    );
    assert!(
        people
            .iter()
            .any(|credit| credit.person_type == "Director" && credit.person.name == "John Director"),
        "NFO director must be linked"
    );

    let incremental_genre = format!("Incremental Genre {}", Uuid::new_v4().simple());
    std::fs::write(
        library_root.join("Scan Movie.nfo"),
        format!(
            "<movie><title>Scan Movie</title><genre>Drama</genre><genre>{incremental_genre}</genre></movie>"
        ),
    )
    .expect("tag-free movie NFO write");
    scan.scan_collection(collection_id)
        .await
        .expect("single-library scan");
    assert!(
        items
            .get_by_type_and_name("Genre", &incremental_genre)
            .await
            .expect("incremental genre entity lookup")
            .is_some(),
        "single-library scans must run the same post-scan reconciliation"
    );
    assert!(
        metadata_root
            .join("Genre")
            .join(&incremental_genre)
            .is_dir(),
        "single-library reconciliation must create the item-by-name directory"
    );
    let movie = items
        .get(movie.id)
        .await
        .expect("rescanned movie lookup")
        .expect("rescanned movie item");
    let tags = values
        .values_for_item(movie.id, ItemValueType::Tags)
        .await
        .expect("rescanned tag query");
    assert_eq!(
        tags.iter()
            .map(|value| value.value.as_str())
            .collect::<Vec<_>>(),
        ["Favorite", "Neo-Noir"],
        "an empty NFO tag response must not clear existing tags"
    );
    assert_eq!(
        movie
            .data
            .as_ref()
            .and_then(|data| data.get("Tags"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>(),
        ["Favorite", "Neo-Noir"]
    );

    let blocked_genre = format!("Blocked Genre {}", Uuid::new_v4().simple());
    let blocked_metadata_root = storage_root.join("blocked-metadata");
    std::fs::write(&blocked_metadata_root, b"not a directory").expect("blocked metadata fixture");
    scan.set_item_by_name_directories(&storage_root, &blocked_metadata_root);
    std::fs::write(
        library_root.join("Scan Movie.nfo"),
        format!("<movie><title>Scan Movie</title><genre>{blocked_genre}</genre></movie>"),
    )
    .expect("blocked genre NFO write");
    scan.scan_collection(collection_id)
        .await
        .expect_err("an unusable metadata root must fail reconciliation");
    assert!(
        items
            .get_by_type_and_name("Genre", &blocked_genre)
            .await
            .expect("blocked genre lookup")
            .is_none(),
        "a genre row must not be inserted when its directory cannot be created"
    );

    std::fs::remove_dir_all(library_root).expect("movie fixture cleanup");
    std::fs::remove_dir_all(storage_root).expect("item-by-name fixture cleanup");
    database.close().await.expect("database pool cleanup");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
