use std::collections::HashSet;

use jellyfin_data::{BaseItemRepository, DatabaseConfig, NewBaseItem};
use sea_orm::ConnectionTrait;
use serde_json::json;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_metadata_candidates_";

#[tokio::test]
async fn missing_metadata_candidates_use_one_lightweight_recursive_query() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary metadata-candidate database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_candidates(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary metadata-candidate database cleanup must succeed");
    administrator.close().await.expect("admin pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("metadata-candidate test task was cancelled: {error}");
    }
}

#[allow(clippy::too_many_lines)]
async fn exercise_candidates(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let repository = BaseItemRepository::new(database.clone());

    let root = create_item(&repository, "Folder", None, true, Some("root"), None, None).await;
    let scoped = create_item(
        &repository,
        "Folder",
        Some(root.id),
        true,
        Some("scope"),
        None,
        None,
    )
    .await;
    let nested = create_item(
        &repository,
        "Folder",
        Some(scoped.id),
        true,
        Some("nested"),
        None,
        None,
    )
    .await;
    let outside = create_item(
        &repository,
        "Folder",
        Some(root.id),
        true,
        Some("outside"),
        None,
        None,
    )
    .await;

    let complete_series = create_item(
        &repository,
        "Series",
        Some(scoped.id),
        true,
        Some("complete series"),
        Some(json!({ "ProviderIds": { "tMdB": "101" } })),
        None,
    )
    .await;
    let missing_overview_series = create_item(
        &repository,
        "Series",
        Some(scoped.id),
        true,
        None,
        Some(json!({ "ProviderIds": { "Tmdb": "102" } })),
        None,
    )
    .await;
    let missing_tmdb_movie = create_item(
        &repository,
        "Movie",
        Some(nested.id),
        false,
        Some("has overview"),
        Some(json!({ "ProviderIds": { "Imdb": "tt1234567" } })),
        None,
    )
    .await;
    let missing_episode = create_item(
        &repository,
        "Episode",
        Some(nested.id),
        false,
        None,
        Some(json!({ "ProviderIds": { "Tmdb": "103" } })),
        Some(complete_series.id),
    )
    .await;
    let malformed_ids_movie = create_item(
        &repository,
        "Movie",
        Some(scoped.id),
        false,
        Some("has overview"),
        Some(json!({ "ProviderIds": "not-an-object" })),
        None,
    )
    .await;
    let complete_movie = create_item(
        &repository,
        "Movie",
        Some(scoped.id),
        false,
        Some("complete"),
        Some(json!({ "ProviderIds": { "Tmdb": "104" } })),
        None,
    )
    .await;
    let outside_movie = create_item(
        &repository,
        "Movie",
        Some(outside.id),
        false,
        None,
        None,
        None,
    )
    .await;

    let scoped_candidates = repository
        .missing_metadata_refresh_candidates(Some(scoped.id))
        .await
        .expect("scoped candidate query");
    let scoped_ids = scoped_candidates
        .iter()
        .map(|candidate| candidate.id)
        .collect::<HashSet<_>>();
    assert_eq!(
        scoped_ids,
        HashSet::from([
            missing_overview_series.id,
            missing_tmdb_movie.id,
            missing_episode.id,
            malformed_ids_movie.id,
        ])
    );
    assert!(!scoped_ids.contains(&complete_series.id));
    assert!(!scoped_ids.contains(&complete_movie.id));
    assert!(!scoped_ids.contains(&outside_movie.id));
    assert_eq!(
        scoped_candidates
            .iter()
            .find(|candidate| candidate.id == missing_episode.id)
            .and_then(|candidate| candidate.series_id),
        Some(complete_series.id)
    );

    let all_ids = repository
        .missing_metadata_refresh_candidates(None)
        .await
        .expect("unscoped candidate query")
        .into_iter()
        .map(|candidate| candidate.id)
        .collect::<HashSet<_>>();
    assert!(all_ids.contains(&outside_movie.id));

    database.close().await.expect("database pool cleanup");
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    overview: Option<&str>,
    data: Option<serde_json::Value>,
    series_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item.overview = overview.map(str::to_owned);
    item.data = data;
    item.series_id = series_id;
    repository.create(item).await.expect("candidate fixture")
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
