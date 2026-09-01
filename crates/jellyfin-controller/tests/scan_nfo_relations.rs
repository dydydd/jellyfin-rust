use jellyfin_controller::{LibraryScanService, VirtualFolderService};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, ItemValueRepository, PersonRepository,
    entities::item_value::ItemValueType,
};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_scan_nfo_relations_";

#[tokio::test]
async fn movie_scan_links_nfo_genres_studios_and_people() {
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
    std::fs::create_dir_all(&library_root).expect("movie fixture directory");
    std::fs::write(library_root.join("Scan Movie.mkv"), b"not a real movie").unwrap();
    std::fs::write(
        library_root.join("Scan Movie.nfo"),
        r#"<?xml version="1.0" encoding="utf-8"?>
<movie>
  <title>Scan Movie</title>
  <genre>Drama</genre>
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

    let scan = LibraryScanService::with_probe_path(database.clone(), "missing-ffprobe");
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
