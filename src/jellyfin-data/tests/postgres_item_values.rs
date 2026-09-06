use chrono::Utc;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, ItemValueError, ItemValueQuery, ItemValueRepository,
    NewBaseItem, NewItemByNameEntity,
    entities::{item_value, item_value_map},
};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn persisted_item_by_name_values_fold_before_counting_and_paging() {
    let database = prepare_database().await;
    let items = BaseItemRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let first_name = format!("A Folded Genre {suffix}");
    let second_name = format!("B Paged Genre {suffix}");
    let movie = create_item(&items, "Movie", &format!("Fold Movie {suffix}")).await;
    let first_value = values
        .link(movie.id, item_value::ItemValueType::Genre, &first_name)
        .await
        .expect("first genre link");
    values
        .link(movie.id, item_value::ItemValueType::Genre, &second_name)
        .await
        .expect("second genre link");

    let mut first_ids = [Uuid::new_v4(), Uuid::new_v4()];
    first_ids.sort_unstable();
    let shared_key = format!("Genre-Folded-{suffix}");
    create_item_by_name(&items, first_ids[0], "Genre", &first_name, &shared_key).await;
    create_item_by_name(
        &items,
        first_ids[1],
        "MediaBrowser.Controller.Entities.Genre",
        &first_name,
        &shared_key,
    )
    .await;
    let second_id = Uuid::new_v4();
    create_item_by_name(
        &items,
        second_id,
        "MediaBrowser.Controller.Entities.Genre",
        &second_name,
        &format!("Genre-Paged-{suffix}"),
    )
    .await;

    let first_page = values
        .query_persisted_item_by_name_values(
            item_value::ItemValueType::Genre,
            "Genre",
            &ItemValueQuery {
                by_name_item_type: Some("Genre".to_owned()),
                limit: Some(1),
                ..ItemValueQuery::default()
            },
        )
        .await
        .expect("first persisted genre page");
    assert_eq!(first_page.total_record_count, 2);
    assert_eq!(first_page.values.len(), 1);
    assert_eq!(first_page.values[0].id, first_ids[0]);
    assert_ne!(first_page.values[0].id, first_value.item_value_id);

    let second_page = values
        .query_persisted_item_by_name_values(
            item_value::ItemValueType::Genre,
            "Genre",
            &ItemValueQuery {
                by_name_item_type: Some("Genre".to_owned()),
                start_index: 1,
                limit: Some(1),
                ..ItemValueQuery::default()
            },
        )
        .await
        .expect("second persisted genre page");
    assert_eq!(second_page.total_record_count, 2);
    assert_eq!(second_page.values.len(), 1);
    assert_eq!(second_page.values[0].id, second_id);

    let candidate_tag = format!("Only Larger Candidate {suffix}");
    let candidate_genre = format!("Candidate Metadata Genre {suffix}");
    let candidate_studio = format!("Candidate Metadata Studio {suffix}");
    values
        .link(
            first_ids[1],
            item_value::ItemValueType::Tags,
            &candidate_tag,
        )
        .await
        .expect("candidate metadata tag");
    values
        .link(
            first_ids[1],
            item_value::ItemValueType::Genre,
            &candidate_genre,
        )
        .await
        .expect("candidate metadata genre");
    values
        .link(
            first_ids[1],
            item_value::ItemValueType::Studios,
            &candidate_studio,
        )
        .await
        .expect("candidate metadata studio");
    let genre_reference = create_item(
        &items,
        "Folder",
        &format!("Candidate Metadata Genre {suffix}"),
    )
    .await;
    let studio_reference = create_item(
        &items,
        "Folder",
        &format!("Candidate Metadata Studio {suffix}"),
    )
    .await;
    let mut larger_candidate = items
        .get(first_ids[1])
        .await
        .expect("larger candidate lookup")
        .expect("larger candidate");
    larger_candidate.production_year = Some(2026);
    larger_candidate.official_rating = Some("PG-13".to_owned());
    items
        .update(larger_candidate)
        .await
        .expect("candidate metadata update");

    for mut query in [
        ItemValueQuery {
            tags: vec![candidate_tag],
            ..ItemValueQuery::default()
        },
        ItemValueQuery {
            genres: vec![candidate_genre],
            ..ItemValueQuery::default()
        },
        ItemValueQuery {
            genre_ids: vec![genre_reference.id],
            ..ItemValueQuery::default()
        },
        ItemValueQuery {
            studios: vec![candidate_studio],
            ..ItemValueQuery::default()
        },
        ItemValueQuery {
            studio_ids: vec![studio_reference.id],
            ..ItemValueQuery::default()
        },
        ItemValueQuery {
            years: vec![2026],
            ..ItemValueQuery::default()
        },
        ItemValueQuery {
            official_ratings: vec!["PG-13".to_owned()],
            ..ItemValueQuery::default()
        },
    ] {
        query.by_name_item_type = Some("Genre".to_owned());
        let metadata_filtered = values
            .query_persisted_item_by_name_values(item_value::ItemValueType::Genre, "Genre", &query)
            .await
            .expect("candidate-filtered persisted genre page");
        assert_eq!(metadata_filtered.total_record_count, 1);
        assert_eq!(metadata_filtered.values.len(), 1);
        assert_eq!(metadata_filtered.values[0].id, first_ids[1]);
    }

    let null_key_name = format!("C Null Key Genre {suffix}");
    values
        .link(movie.id, item_value::ItemValueType::Genre, &null_key_name)
        .await
        .expect("null-key genre link");
    let mut null_key_ids = [Uuid::new_v4(), Uuid::new_v4()];
    null_key_ids.sort_unstable();
    for id in null_key_ids {
        let mut item = NewBaseItem::new(id, "Genre");
        item.name = Some(null_key_name.clone());
        item.sort_name = Some(null_key_name.clone());
        item.is_folder = true;
        items
            .create(item)
            .await
            .expect("null presentation-key genre");
    }
    let null_key_page = values
        .query_persisted_item_by_name_values(
            item_value::ItemValueType::Genre,
            "Genre",
            &ItemValueQuery {
                by_name_item_type: Some("Genre".to_owned()),
                search_term: Some(null_key_name),
                ..ItemValueQuery::default()
            },
        )
        .await
        .expect("null presentation-key page");
    assert_eq!(null_key_page.total_record_count, 1);
    assert_eq!(null_key_page.values[0].id, null_key_ids[0]);

    items
        .delete_many(&[
            movie.id,
            first_ids[0],
            first_ids[1],
            second_id,
            genre_reference.id,
            studio_reference.id,
            null_key_ids[0],
            null_key_ids[1],
        ])
        .await
        .expect("fixture cleanup");
}

#[tokio::test]
async fn genre_entity_reconciliation_is_set_based_and_preserves_existing_rows() {
    let database = prepare_database().await;
    let items = BaseItemRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("Shared Genre {suffix}");
    let movie = create_item(&items, "Movie", &format!("Movie {suffix}")).await;
    let audio = create_item(&items, "Audio", &format!("Audio {suffix}")).await;
    values
        .link(movie.id, item_value::ItemValueType::Genre, &name)
        .await
        .expect("generic genre link");
    values
        .link(audio.id, item_value::ItemValueType::Genre, &name)
        .await
        .expect("music genre link");

    let required = values
        .required_genre_entities_page(None, 512)
        .await
        .expect("required genre entities");
    assert!(
        required
            .iter()
            .any(|entry| { entry.item_type == "Genre" && entry.name == name })
    );
    assert!(
        required
            .iter()
            .any(|entry| { entry.item_type == "MusicGenre" && entry.name == name })
    );

    let mut existing = NewBaseItem::new(Uuid::new_v4(), "Genre");
    existing.name = Some(name.clone());
    existing.overview = Some("keep existing metadata".to_owned());
    existing.is_folder = true;
    let existing = items.create(existing).await.expect("existing genre");
    let rejected_duplicate_id = Uuid::new_v4();
    let music_genre_id = Uuid::new_v4();
    let candidates = [
        NewItemByNameEntity {
            id: rejected_duplicate_id,
            item_type: "Genre".to_owned(),
            name: name.clone(),
            path: format!("metadata/Genre/{name}"),
            presentation_unique_key: format!("Genre-{name}"),
            date_created: Utc::now(),
            date_modified: Utc::now(),
        },
        NewItemByNameEntity {
            id: music_genre_id,
            item_type: "MusicGenre".to_owned(),
            name: name.clone(),
            path: format!("metadata/MusicGenre/{name}"),
            presentation_unique_key: format!("MusicGenre-{name}"),
            date_created: Utc::now(),
            date_modified: Utc::now(),
        },
    ];

    assert_eq!(
        items
            .create_missing_item_by_name_entities(&candidates)
            .await
            .expect("first reconciliation"),
        1
    );
    assert_eq!(
        items
            .create_missing_item_by_name_entities(&candidates)
            .await
            .expect("idempotent reconciliation"),
        0
    );
    assert!(items.get(rejected_duplicate_id).await.unwrap().is_none());
    assert_eq!(
        items
            .get(existing.id)
            .await
            .unwrap()
            .unwrap()
            .overview
            .as_deref(),
        Some("keep existing metadata")
    );
    let inserted = items.get(music_genre_id).await.unwrap().unwrap();
    assert_eq!(inserted.name.as_deref(), Some(name.as_str()));
    assert!(inserted.is_folder);
    assert_genre_no_longer_required(&values, &name).await;

    assert_item_by_name_index(&database).await;

    items
        .delete_many(&[movie.id, audio.id, existing.id, music_genre_id])
        .await
        .expect("fixture cleanup");
}

async fn assert_genre_no_longer_required(values: &ItemValueRepository, name: &str) {
    assert!(
        values
            .required_genre_entities_page(None, 512)
            .await
            .expect("post-reconciliation requirements")
            .iter()
            .all(|entry| entry.name != name),
        "existing canonical item-by-name rows must not be prepared again"
    );
}

async fn assert_item_by_name_index(database: &DatabaseConnection) {
    let index = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' \
               AND indexname = 'base_items_item_by_name_type_clean_name_idx'"
                .to_owned(),
        ))
        .await
        .expect("index catalog query")
        .expect("item-by-name index");
    let definition = String::try_get(&index, "", "indexdef").expect("index definition");
    assert!(!definition.contains("UNIQUE"));
    assert!(definition.contains("clean_name"));
}
use jellyfin_migration::CreateItemValuesMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryResult, Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_item_values_vertical_slice() {
    let database = prepare_database().await;
    let items = BaseItemRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let fixtures = Box::pin(assert_lookup_links_and_concurrency(
        &database, &items, &values,
    ))
    .await;
    assert_postgres_catalog(&database).await;
    assert_postgres_query_plans(&database, &fixtures).await;
    cleanup(&items, fixtures).await;
}

struct Fixtures {
    item_ids: Vec<Uuid>,
    first_item_id: Uuid,
    genre_id: Uuid,
    exact_value: String,
    clean_value: String,
}

struct SeededGenre {
    item_ids: Vec<Uuid>,
    first_item_id: Uuid,
    genre: item_value::Model,
    exact_value: String,
    normalized: String,
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreateItemValuesMigration
        .up(&schema)
        .await
        .expect("reapplying item-value DDL must succeed");
    CreateItemValuesMigration
        .up(&schema)
        .await
        .expect("item-value DDL must remain idempotent");
    database
}

async fn assert_lookup_links_and_concurrency(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    values: &ItemValueRepository,
) -> Fixtures {
    assert_invalid_values(values).await;
    let mut seeded = seed_normalized_genre(items, values).await;
    assert_bidirectional_lookups(values, &seeded).await;
    let concurrent_ids = Box::pin(assert_concurrent_deduplication(
        database, items, values, &seeded,
    ))
    .await;
    seeded.item_ids.extend(concurrent_ids);
    Fixtures {
        item_ids: seeded.item_ids,
        first_item_id: seeded.first_item_id,
        genre_id: seeded.genre.item_value_id,
        exact_value: seeded.exact_value,
        clean_value: seeded.genre.clean_value,
    }
}

async fn assert_invalid_values(values: &ItemValueRepository) {
    assert!(matches!(
        values.upsert(item_value::ItemValueType::Genre, "   ").await,
        Err(ItemValueError::InvalidValue)
    ));
    assert!(matches!(
        values.upsert(item_value::ItemValueType::Genre, "---").await,
        Err(ItemValueError::InvalidValue)
    ));
    assert!(matches!(
        values
            .link(Uuid::new_v4(), item_value::ItemValueType::Genre, "Missing")
            .await,
        Err(ItemValueError::ItemNotFound)
    ));
}

async fn seed_normalized_genre(
    items: &BaseItemRepository,
    values: &ItemValueRepository,
) -> SeededGenre {
    let first = create_item(items, "Audio", "First Track").await;
    let second = create_item(items, "MusicAlbum", "Album").await;
    let non_music = create_item(items, "Book", "Book").await;
    let exact_value = format!("Électronique {}", Uuid::new_v4().simple());
    let normalized = exact_value
        .replace('É', "e")
        .to_uppercase()
        .replace(' ', "---");
    let first_genre = values
        .link(first.id, item_value::ItemValueType::Genre, &exact_value)
        .await
        .expect("first genre link");
    let normalized_genre = values
        .link(second.id, item_value::ItemValueType::Genre, &normalized)
        .await
        .expect("normalized genre link");
    assert_eq!(normalized_genre.item_value_id, first_genre.item_value_id);
    values
        .link(non_music.id, item_value::ItemValueType::Genre, &exact_value)
        .await
        .expect("non-music genre link");
    SeededGenre {
        item_ids: vec![first.id, second.id, non_music.id],
        first_item_id: first.id,
        genre: first_genre,
        exact_value,
        normalized,
    }
}

async fn assert_bidirectional_lookups(values: &ItemValueRepository, seeded: &SeededGenre) {
    let exact = values
        .get_exact(item_value::ItemValueType::Genre, &seeded.exact_value)
        .await
        .expect("exact lookup")
        .expect("exact value");
    assert_eq!(exact.item_value_id, seeded.genre.item_value_id);
    assert!(
        values
            .get_exact(item_value::ItemValueType::Genre, &seeded.normalized)
            .await
            .expect("variant exact lookup")
            .is_none()
    );
    let normalized_lookup = values
        .get_normalized(item_value::ItemValueType::Genre, &seeded.normalized)
        .await
        .expect("normalized lookup")
        .expect("normalized value");
    assert_eq!(normalized_lookup.item_value_id, seeded.genre.item_value_id);
    assert_eq!(normalized_lookup.value, seeded.exact_value);

    let first_values = values
        .values_for_item(seeded.first_item_id, item_value::ItemValueType::Genre)
        .await
        .expect("values for item");
    assert_eq!(first_values, vec![seeded.genre.clone()]);
    let linked_items = values
        .items_for_value(item_value::ItemValueType::Genre, &seeded.normalized)
        .await
        .expect("items for value");
    assert_eq!(linked_items.len(), 3);
}

async fn assert_concurrent_deduplication(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    values: &ItemValueRepository,
    seeded: &SeededGenre,
) -> Vec<Uuid> {
    let concurrent_items = [
        create_item(items, "Audio", "Concurrent One").await,
        create_item(items, "Audio", "Concurrent Two").await,
        create_item(items, "MusicVideo", "Concurrent Three").await,
        create_item(items, "MusicArtist", "Concurrent Four").await,
    ];
    let (one, two, three, four) = tokio::join!(
        values.link(
            concurrent_items[0].id,
            item_value::ItemValueType::Genre,
            &seeded.exact_value
        ),
        values.link(
            concurrent_items[1].id,
            item_value::ItemValueType::Genre,
            &seeded.normalized
        ),
        values.link(
            concurrent_items[2].id,
            item_value::ItemValueType::Genre,
            &seeded.exact_value
        ),
        values.link(
            concurrent_items[3].id,
            item_value::ItemValueType::Genre,
            &seeded.normalized
        ),
    );
    for result in [one, two, three, four] {
        assert_eq!(
            result.expect("concurrent link").item_value_id,
            seeded.genre.item_value_id
        );
    }
    let normalized_count = item_value::Entity::find()
        .filter(item_value::Column::ValueType.eq(item_value::ItemValueType::Genre))
        .filter(item_value::Column::CleanValue.eq(&seeded.genre.clean_value))
        .count(database)
        .await
        .expect("normalized count");
    assert_eq!(normalized_count, 1);
    let link_count = item_value_map::Entity::find()
        .filter(item_value_map::Column::ItemValueId.eq(seeded.genre.item_value_id))
        .count(database)
        .await
        .expect("link count");
    assert_eq!(link_count, 7);
    concurrent_items.map(|item| item.id).to_vec()
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = catalog_names(
        database,
        "SELECT conname AS name FROM pg_constraint \
         WHERE conrelid IN ('jellyfin.item_values'::regclass, 'jellyfin.item_value_map'::regclass)",
    )
    .await;
    for expected in [
        "item_values_type_valid",
        "item_values_value_not_empty",
        "item_values_clean_value_not_empty",
        "item_value_map_value_fkey",
        "item_value_map_item_fkey",
    ] {
        assert!(constraints.iter().any(|name| name == expected));
    }
    let indexes = catalog_names(
        database,
        "SELECT indexname AS name FROM pg_indexes \
         WHERE schemaname = 'jellyfin' \
           AND tablename IN ('item_values', 'item_value_map')",
    )
    .await;
    for expected in [
        "item_values_type_value_key",
        "item_values_type_clean_value_key",
        "item_value_map_pkey",
        "item_value_map_item_idx",
    ] {
        assert!(indexes.iter().any(|name| name == expected));
    }
}

async fn assert_postgres_query_plans(database: &DatabaseConnection, fixtures: &Fixtures) {
    let transaction = database.begin().await.expect("explain transaction");
    transaction
        .execute_unprepared("ANALYZE jellyfin.item_values; ANALYZE jellyfin.item_value_map")
        .await
        .expect("refresh item-value planner statistics");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");
    let plans = [
        (
            "item_values_type_value_key",
            explain_text(
                &transaction,
                "EXPLAIN (FORMAT TEXT) SELECT item_value_id, value, clean_value \
                 FROM jellyfin.item_values WHERE type = 2 AND value = $1",
                &fixtures.exact_value,
            )
            .await,
        ),
        (
            "item_values_type_clean_value_key",
            explain_text(
                &transaction,
                "EXPLAIN (FORMAT TEXT) SELECT item_value_id, value, clean_value \
                 FROM jellyfin.item_values WHERE type = 2 AND clean_value = $1",
                &fixtures.clean_value,
            )
            .await,
        ),
        (
            "item_value_map_item_idx",
            explain_uuid(
                &transaction,
                "EXPLAIN (FORMAT TEXT) SELECT item_value_id FROM jellyfin.item_value_map \
                 WHERE item_id = $1",
                fixtures.first_item_id,
            )
            .await,
        ),
        (
            "item_value_map_pkey",
            explain_uuid(
                &transaction,
                "EXPLAIN (FORMAT TEXT) SELECT item_id FROM jellyfin.item_value_map \
                 WHERE item_value_id = $1",
                fixtures.genre_id,
            )
            .await,
        ),
    ];
    for (index, plan) in plans {
        assert!(plan.contains(index), "expected {index} in plan:\n{plan}");
    }
    transaction.rollback().await.expect("explain rollback");
}

async fn cleanup(items: &BaseItemRepository, fixtures: Fixtures) {
    for item_id in fixtures.item_ids {
        items.delete(item_id).await.expect("item cleanup");
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    repository.create(item).await.expect("base item creation")
}

async fn create_item_by_name(
    repository: &BaseItemRepository,
    id: Uuid,
    item_type: &str,
    name: &str,
    presentation_unique_key: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.is_folder = true;
    item.presentation_unique_key = Some(presentation_unique_key.to_owned());
    repository
        .create(item)
        .await
        .expect("item-by-name creation")
}

async fn catalog_names(database: &DatabaseConnection, sql: &str) -> Vec<String> {
    database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            sql.to_owned(),
        ))
        .await
        .expect("PostgreSQL catalog query")
        .into_iter()
        .map(|row| String::try_get(&row, "", "name").expect("catalog name must be text"))
        .collect()
}

async fn explain_text(
    transaction: &sea_orm::DatabaseTransaction,
    sql: &str,
    value: &str,
) -> String {
    explain(
        transaction
            .query_all(Statement::from_sql_and_values(
                transaction.get_database_backend(),
                sql,
                [value.into()],
            ))
            .await
            .expect("text EXPLAIN query"),
    )
}

async fn explain_uuid(
    transaction: &sea_orm::DatabaseTransaction,
    sql: &str,
    value: Uuid,
) -> String {
    explain(
        transaction
            .query_all(Statement::from_sql_and_values(
                transaction.get_database_backend(),
                sql,
                [value.into()],
            ))
            .await
            .expect("uuid EXPLAIN query"),
    )
}

fn explain(rows: Vec<QueryResult>) -> String {
    rows.into_iter()
        .map(|row| String::try_get(&row, "", "QUERY PLAN").expect("EXPLAIN line must be text"))
        .collect::<Vec<_>>()
        .join("\n")
}
