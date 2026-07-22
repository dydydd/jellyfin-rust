use jellyfin_data::{BaseItemQuery, BaseItemRepository, DatabaseConfig, NewBaseItem};
use jellyfin_migration::NormalizeBaseItemSearchMigration;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::Value;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static TEST_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn clean_name_keeps_punctuation_and_search_without_punctuation_passes() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database.clone());
    assert_historical_rows_were_generated(&repository).await;

    let item = create_item(&repository, "Mr. Robot").await;
    assert_eq!(item.clean_name.as_deref(), Some("mr robot"));
    assert_search_contains(&repository, "Mr Robot", item.id).await;

    let literal = create_item(&repository, "A 100%_Ready").await;
    let wildcard_decoy = create_item(&repository, "B 100XYReady").await;
    let page = repository
        .query(&BaseItemQuery {
            search_term: Some("100%_ready".to_owned()),
            ..Default::default()
        })
        .await
        .expect("literal wildcard search must succeed");
    assert!(
        page.items
            .iter()
            .any(|candidate| candidate.id == literal.id)
    );
    assert!(
        !page
            .items
            .iter()
            .any(|candidate| candidate.id == wildcard_decoy.id)
    );

    let mut updated = item;
    updated.name = Some("Café Society".to_owned());
    let updated = repository
        .update(updated)
        .await
        .expect("generated clean name must update with display name");
    assert_eq!(updated.clean_name.as_deref(), Some("cafe society"));
    assert_search_contains(&repository, "Cafe Society", updated.id).await;

    assert_postgres_search_schema_and_plan(&database).await;
    cleanup(&repository, &[updated.id, literal.id, wildcard_decoy.id]).await;
}

#[tokio::test]
async fn clean_name_normalizes_various_punctuation() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database);
    let cases = [
        ("Spider-Man: Homecoming", "spider man homecoming"),
        ("Beyoncé — Live!", "beyonce live"),
        ("Hello, World!", "hello world"),
        (
            "(The) Good, the Bad & the Ugly",
            "the good the bad the ugly",
        ),
        ("Wall-E", "wall e"),
        ("No. 1: The Beginning", "no 1 the beginning"),
        ("Café-au-lait", "cafe au lait"),
    ];
    let mut ids = Vec::with_capacity(cases.len());

    for (title, expected_clean) in cases {
        let item = create_item(&repository, title).await;
        assert_eq!(item.clean_name.as_deref(), Some(expected_clean));
        assert_search_contains(&repository, expected_clean, item.id).await;
        ids.push(item.id);
    }

    cleanup(&repository, &ids).await;
}

#[tokio::test]
async fn clean_name_normalizes_titles_with_slashes() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database);
    let cases = [("Face/Off", "face off"), ("V/H/S", "v h s")];
    let mut ids = Vec::with_capacity(cases.len());

    for (title, expected_clean) in cases {
        let item = create_item(&repository, title).await;
        assert_eq!(item.clean_name.as_deref(), Some(expected_clean));
        assert_search_contains(&repository, expected_clean, item.id).await;
        ids.push(item.id);
    }

    cleanup(&repository, &ids).await;
}

async fn prepare_database() -> DatabaseConnection {
    static DATABASE: OnceCell<DatabaseConnection> = OnceCell::const_new();
    DATABASE
        .get_or_init(|| async {
            let config = DatabaseConfig {
                max_connections: 1,
                ..DatabaseConfig::default()
            };
            let database = jellyfin_data::connect(&config)
                .await
                .expect("local PostgreSQL must be available");
            jellyfin_data::migrate(&database)
                .await
                .expect("PostgreSQL migrations must succeed");
            let schema = SchemaManager::new(&database);
            NormalizeBaseItemSearchMigration
                .up(&schema)
                .await
                .expect("reapplying normalized search DDL must succeed");
            NormalizeBaseItemSearchMigration
                .up(&schema)
                .await
                .expect("normalized search DDL must remain idempotent");
            database
        })
        .await
        .clone()
}

async fn create_item(
    repository: &BaseItemRepository,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Series");
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    repository.create(item).await.expect("search item creation")
}

async fn assert_search_contains(repository: &BaseItemRepository, term: &str, expected_id: Uuid) {
    let page = repository
        .query(&BaseItemQuery {
            search_term: Some(term.to_owned()),
            ..Default::default()
        })
        .await
        .expect("normalized search must succeed");
    assert!(
        page.items.iter().any(|item| item.id == expected_id),
        "search for {term:?} must return {expected_id}"
    );
}

async fn assert_historical_rows_were_generated(repository: &BaseItemRepository) {
    let placeholder = repository
        .get(Uuid::from_u128(1))
        .await
        .expect("placeholder lookup must succeed")
        .expect("base-item migration must seed placeholder before clean-name migration");
    assert_eq!(
        placeholder.clean_name.as_deref(),
        Some(
            "this is a placeholder item for userdata that has been detached from its original item"
        )
    );
}

async fn assert_postgres_search_schema_and_plan(database: &DatabaseConnection) {
    let generated = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT is_generated, generation_expression \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' \
               AND table_name = 'base_items' \
               AND column_name = 'clean_name'"
                .to_owned(),
        ))
        .await
        .expect("generated-column catalog query must succeed")
        .expect("clean_name catalog row must exist");
    assert_eq!(
        String::try_get(&generated, "", "is_generated").expect("generated marker"),
        "ALWAYS"
    );
    let expression =
        String::try_get(&generated, "", "generation_expression").expect("generation expression");
    assert!(expression.contains("normalize_search_text"));

    let indexes = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' \
               AND tablename = 'base_items' \
               AND indexname IN ('base_items_name_trgm_idx', \
                                 'base_items_clean_name_trgm_idx')"
                .to_owned(),
        ))
        .await
        .expect("search-index catalog query must succeed")
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name"))
        .collect::<Vec<_>>();
    assert!(
        indexes
            .iter()
            .any(|index| index == "base_items_clean_name_trgm_idx")
    );
    assert!(
        !indexes
            .iter()
            .any(|index| index == "base_items_name_trgm_idx")
    );

    let transaction = database.begin().await.expect("search plan transaction");
    let suffix = Uuid::new_v4().simple().to_string();
    transaction
        .execute(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            r"INSERT INTO jellyfin.base_items (id, item_type, name, sort_name)
              SELECT md5($1 || '-search-' || value::text)::uuid,
                     'Series',
                     CASE WHEN value <= 8
                          THEN 'Beyoncé — Rare Needle ' || $1 || ' ' || value::text
                          ELSE 'Planner punctuation noise ' || value::text END,
                     'Planner ' || value::text
                FROM generate_series(1, 4096) AS value",
            [suffix.clone().into()],
        ))
        .await
        .expect("search planner fixture insert must succeed");
    transaction
        .execute_unprepared("ANALYZE jellyfin.base_items; SET LOCAL enable_seqscan = off")
        .await
        .expect("search planner statistics must refresh");
    let row = transaction
        .query_one(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            "EXPLAIN (FORMAT JSON) SELECT id FROM jellyfin.base_items \
             WHERE clean_name ILIKE $1",
            [format!("%rare needle {suffix}%").into()],
        ))
        .await
        .expect("JSON search explain must succeed")
        .expect("JSON search explain must return a row");
    let plan = Value::try_get(&row, "", "QUERY PLAN").expect("JSON search plan must decode");
    let serialized = plan.to_string();
    assert!(
        serialized.contains("base_items_clean_name_trgm_idx"),
        "normalized search must use its trigram index: {serialized}"
    );
    transaction.rollback().await.expect("search plan rollback");
}

async fn cleanup(repository: &BaseItemRepository, ids: &[Uuid]) {
    repository
        .delete_many(ids)
        .await
        .expect("search punctuation fixtures must clean up");
}
