use std::collections::HashSet;

use chrono::Utc;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig, NewBaseItem,
    NewBaseItemImage,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait, TryGetable,
};
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
        Box::pin(exercise_candidates(&task_database_name)).await;
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

    let mut complete_series = create_item(
        &repository,
        "Series",
        Some(scoped.id),
        true,
        Some("complete series"),
        Some(json!({ "ProviderIds": { "tMdB": "101" } })),
        None,
    )
    .await;
    complete_series.name = Some("Complete Series".to_owned());
    complete_series.sort_name = complete_series.name.clone();
    complete_series = repository
        .update(complete_series)
        .await
        .expect("named series update");
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
    let mut fallback_title_episode = create_item(
        &repository,
        "Episode",
        Some(nested.id),
        false,
        Some("complete episode overview"),
        Some(json!({ "ProviderIds": { "Tmdb": "107" } })),
        Some(complete_series.id),
    )
    .await;
    fallback_title_episode.name = complete_series.name.clone();
    fallback_title_episode.sort_name = fallback_title_episode.name.clone();
    fallback_title_episode = repository
        .update(fallback_title_episode)
        .await
        .expect("fallback-title episode update");
    let empty_title_episode = create_item(
        &repository,
        "Episode",
        Some(nested.id),
        false,
        Some("complete episode overview"),
        Some(json!({ "ProviderIds": { "Tmdb": "108" } })),
        Some(complete_series.id),
    )
    .await;
    let mut established_title_episode = create_item(
        &repository,
        "Episode",
        Some(nested.id),
        false,
        Some("complete episode overview"),
        Some(json!({ "ProviderIds": { "Tmdb": "109" } })),
        Some(complete_series.id),
    )
    .await;
    established_title_episode.name = Some("Established Episode".to_owned());
    established_title_episode.sort_name = established_title_episode.name.clone();
    established_title_episode = repository
        .update(established_title_episode)
        .await
        .expect("established-title episode update");
    let mut alternate_fallback = NewBaseItem::new(Uuid::new_v4(), "Episode");
    alternate_fallback.parent_id = Some(nested.id);
    alternate_fallback.name = complete_series.name.clone();
    alternate_fallback.sort_name = alternate_fallback.name.clone();
    alternate_fallback.overview = Some("complete alternate overview".to_owned());
    alternate_fallback.data = Some(json!({ "ProviderIds": { "Tmdb": "110" } }));
    alternate_fallback.series_id = Some(complete_series.id);
    alternate_fallback.primary_version_id = Some(fallback_title_episode.id);
    let alternate_fallback_episode = repository
        .create(alternate_fallback)
        .await
        .expect("alternate fallback episode creation");
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
    let image_only_movie = create_item(
        &repository,
        "Movie",
        Some(scoped.id),
        false,
        Some("complete metadata without a poster"),
        Some(json!({ "ProviderIds": { "tMdB": "105" } })),
        None,
    )
    .await;
    let mut alternate_movie = create_item(
        &repository,
        "Movie",
        Some(scoped.id),
        false,
        Some("alternate version"),
        Some(json!({ "ProviderIds": { "Tmdb": "106" } })),
        None,
    )
    .await;
    alternate_movie.primary_version_id = Some(image_only_movie.id);
    repository
        .update(alternate_movie)
        .await
        .expect("alternate movie update");
    BaseItemImageRepository::new(database.clone())
        .replace(
            complete_movie.id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Primary,
                image_index: 0,
                path: "/metadata/complete-movie.jpg".to_owned(),
                date_modified: Utc::now(),
                width: None,
                height: None,
                blurhash: None,
            }],
        )
        .await
        .expect("existing primary image");
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
            fallback_title_episode.id,
            empty_title_episode.id,
            malformed_ids_movie.id,
        ])
    );
    assert!(!scoped_ids.contains(&complete_series.id));
    assert!(!scoped_ids.contains(&complete_movie.id));
    assert!(!scoped_ids.contains(&established_title_episode.id));
    assert!(!scoped_ids.contains(&alternate_fallback_episode.id));
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

    let missing_primary_images = repository
        .missing_primary_image_refresh_candidates(Some(scoped.id))
        .await
        .expect("missing primary image candidate query");
    let missing_primary_ids = missing_primary_images
        .iter()
        .map(|candidate| candidate.id)
        .collect::<HashSet<_>>();
    assert_eq!(
        missing_primary_ids,
        HashSet::from([
            complete_series.id,
            missing_overview_series.id,
            image_only_movie.id,
        ])
    );
    assert!(
        missing_primary_images
            .iter()
            .all(|candidate| matches!(candidate.item_type.as_str(), "Movie" | "Series"))
    );
    assert!(!missing_primary_ids.contains(&complete_movie.id));
    assert!(!missing_primary_ids.contains(&missing_tmdb_movie.id));

    exercise_original_title_repair(&database, &repository, scoped.id).await;

    database.close().await.expect("database pool cleanup");
}

#[allow(clippy::too_many_lines)]
async fn exercise_original_title_repair(
    database: &DatabaseConnection,
    repository: &BaseItemRepository,
    parent_id: Uuid,
) {
    let series = create_named_series(repository, parent_id, "Repair Series").await;
    let other_series = create_named_series(repository, parent_id, "Other Series").await;

    let valid = create_repair_episode(
        repository,
        series.id,
        Some("Repair Series"),
        "/media/Repair Series/S01E01.mkv",
        "Original Episode",
        None,
        None,
    )
    .await;
    let empty_name = create_repair_episode(
        repository,
        series.id,
        None,
        "/media/Repair Series/S01E02.mkv",
        "Original Two",
        None,
        None,
    )
    .await;
    let path_name = create_repair_episode(
        repository,
        series.id,
        Some("S01E03"),
        "/media/Repair Series/S01E03.mkv",
        "Original Three",
        None,
        None,
    )
    .await;
    let locked = create_repair_episode(
        repository,
        series.id,
        Some("Repair Series"),
        "/media/Repair Series/S01E04.mkv",
        "Locked Original",
        Some(json!("Overview|NAME")),
        None,
    )
    .await;
    let comma_locked = create_repair_episode(
        repository,
        series.id,
        Some("Repair Series"),
        "/media/Repair Series/S01E09.mkv",
        "Comma Locked Original",
        Some(json!("Overview,Name")),
        None,
    )
    .await;
    let alternate = create_repair_episode(
        repository,
        series.id,
        Some("Repair Series"),
        "/media/Repair Series/S01E05.mkv",
        "Alternate Original",
        None,
        Some(valid.id),
    )
    .await;
    let series_original = create_repair_episode(
        repository,
        series.id,
        Some("Repair Series"),
        "/media/Repair Series/S01E06.mkv",
        "Repair Series",
        None,
        None,
    )
    .await;
    let path_original = create_repair_episode(
        repository,
        series.id,
        Some("Repair Series"),
        "/media/Repair Series/S01E07.mkv",
        "S01E07",
        None,
        None,
    )
    .await;
    let localized = create_repair_episode(
        repository,
        series.id,
        Some("Localized Episode"),
        "/media/Repair Series/S01E08.mkv",
        "Original Eight",
        None,
        None,
    )
    .await;
    let metadata_series_placeholder = add_episode_data_field(
        repository,
        create_repair_episode(
            repository,
            series.id,
            Some("Metadata Series Name"),
            "/media/Repair Series/S01E10.mkv",
            "Original Ten",
            None,
            None,
        )
        .await,
        "SeriesName",
        json!("Metadata Series Name"),
    )
    .await;
    let metadata_series_original = add_episode_data_field(
        repository,
        create_repair_episode(
            repository,
            series.id,
            Some("Repair Series"),
            "/media/Repair Series/S01E11.mkv",
            "Metadata Series Name",
            None,
            None,
        )
        .await,
        "seriesname",
        json!("Metadata Series Name"),
    )
    .await;
    let mut orphan = create_repair_episode(
        repository,
        series.id,
        Some("S01E12"),
        "/media/Repair Series/S01E12.mkv",
        "Orphan Original",
        None,
        None,
    )
    .await;
    orphan.series_id = None;
    let orphan = repository
        .update(orphan)
        .await
        .expect("orphan episode fixture");
    let other = create_repair_episode(
        repository,
        other_series.id,
        Some("Other Series"),
        "/media/Other Series/S01E01.mkv",
        "Other Original",
        None,
        None,
    )
    .await;

    assert_episode_series_lookup_plan(database, series.id).await;

    assert_eq!(
        repository
            .repair_episode_titles_from_original(series.id)
            .await
            .expect("set-based series title repair"),
        4
    );
    assert_item_name(repository, valid.id, "Original Episode").await;
    assert_item_name(repository, empty_name.id, "Original Two").await;
    assert_item_name(repository, path_name.id, "Original Three").await;
    assert_item_name(repository, locked.id, "Repair Series").await;
    assert_item_name(repository, comma_locked.id, "Repair Series").await;
    assert_item_name(repository, alternate.id, "Repair Series").await;
    assert_item_name(repository, series_original.id, "Repair Series").await;
    assert_item_name(repository, path_original.id, "Repair Series").await;
    assert_item_name(repository, localized.id, "Localized Episode").await;
    assert_item_name(repository, metadata_series_placeholder.id, "Original Ten").await;
    assert_item_name(repository, metadata_series_original.id, "Repair Series").await;
    assert_item_name(repository, orphan.id, "S01E12").await;
    assert_item_name(repository, other.id, "Other Series").await;

    assert_eq!(
        repository
            .repair_episode_titles_from_original(other.id)
            .await
            .expect("set-based single-episode title repair"),
        1
    );
    assert_item_name(repository, other.id, "Other Original").await;

    assert_eq!(
        repository
            .repair_episode_titles_from_original(orphan.id)
            .await
            .expect("set-based orphan-episode title repair"),
        1
    );
    assert_item_name(repository, orphan.id, "Orphan Original").await;
}

async fn assert_episode_series_lookup_plan(database: &DatabaseConnection, series_id: Uuid) {
    let transaction = database.begin().await.expect("EXPLAIN transaction");
    transaction
        .execute_unprepared("ANALYZE jellyfin.base_items; SET LOCAL enable_seqscan = off")
        .await
        .expect("episode repair planner statistics");
    let plan = transaction
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"EXPLAIN (FORMAT TEXT)
               WITH scope AS MATERIALIZED (
                   SELECT item_type FROM jellyfin.base_items WHERE id = $1
               ),
               candidate_ids AS (
                   SELECT episode.id
                   FROM scope
                   JOIN jellyfin.base_items AS episode ON episode.series_id = $1
                   WHERE scope.item_type = 'Series'
                     AND episode.item_type = 'Episode'
                     AND episode.primary_version_id IS NULL
                   UNION ALL
                   SELECT episode.id
                   FROM scope
                   JOIN jellyfin.base_items AS episode ON episode.id = $1
                   WHERE scope.item_type = 'Episode'
                     AND episode.item_type = 'Episode'
                     AND episode.primary_version_id IS NULL
               )
               SELECT id FROM candidate_ids",
            [series_id.into()],
        ))
        .await
        .expect("episode repair EXPLAIN")
        .iter()
        .map(|row| String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("base_items_primary_episode_series_idx"),
        "series-scoped episode repair must use its partial index:\n{plan}"
    );
    transaction.rollback().await.expect("EXPLAIN rollback");
}

async fn create_named_series(
    repository: &BaseItemRepository,
    parent_id: Uuid,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Series");
    item.parent_id = Some(parent_id);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.is_folder = true;
    repository.create(item).await.expect("series fixture")
}

#[allow(clippy::too_many_arguments)]
async fn create_repair_episode(
    repository: &BaseItemRepository,
    series_id: Uuid,
    name: Option<&str>,
    path: &str,
    original_title: &str,
    locked_fields: Option<serde_json::Value>,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut data = json!({ "originaltitle": original_title });
    if let Some(locked_fields) = locked_fields {
        data.as_object_mut()
            .expect("episode metadata object")
            .insert("lockedfields".to_owned(), locked_fields);
    }
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Episode");
    item.parent_id = Some(series_id);
    item.series_id = Some(series_id);
    item.name = name.map(str::to_owned);
    item.sort_name = item.name.clone();
    item.path = Some(path.to_owned());
    item.data = Some(data);
    item.primary_version_id = primary_version_id;
    repository.create(item).await.expect("episode fixture")
}

async fn add_episode_data_field(
    repository: &BaseItemRepository,
    mut item: jellyfin_data::entities::base_item::Model,
    key: &str,
    value: serde_json::Value,
) -> jellyfin_data::entities::base_item::Model {
    item.data
        .get_or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("episode metadata object")
        .insert(key.to_owned(), value);
    repository
        .update(item)
        .await
        .expect("episode metadata fixture")
}

async fn assert_item_name(repository: &BaseItemRepository, item_id: Uuid, expected: &str) {
    let item = repository
        .get(item_id)
        .await
        .expect("episode lookup")
        .expect("episode fixture exists");
    assert_eq!(item.name.as_deref(), Some(expected));
    assert_eq!(item.sort_name.as_deref(), Some(expected));
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
