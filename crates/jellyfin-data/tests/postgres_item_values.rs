use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, ItemValueError, ItemValueRepository, NewBaseItem,
    entities::{item_value, item_value_map},
};
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
