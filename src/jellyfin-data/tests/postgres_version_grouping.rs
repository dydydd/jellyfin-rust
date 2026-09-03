use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, DatabaseConfig, NewBaseItem,
};
use jellyfin_migration::AddAlternateItemVersionsMigration;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::Value;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static TEST_LOCK: Mutex<()> = Mutex::const_new(());

// Official GetItemList_VersionGroup_ReturnsPrimaryVersion.
#[tokio::test]
async fn version_group_returns_primary_version() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database);
    let (alternate_id, primary_id) = ordered_ids();
    let key = format!("version-group-{}", Uuid::new_v4().simple());
    let primary = create_item(&repository, primary_id, "Zulu Primary", Some(&key), None).await;
    let alternate = create_item(
        &repository,
        alternate_id,
        "Alpha Alternate",
        Some(&key),
        Some(primary.id),
    )
    .await;

    let grouped = repository
        .query(&grouped_query(&[primary.id, alternate.id]))
        .await
        .expect("version-group query must succeed");
    assert_eq!(grouped.total_record_count, 1);
    assert_eq!(grouped.items.len(), 1);
    assert_eq!(grouped.items[0].id, primary.id);

    let ungrouped = repository
        .query(&BaseItemQuery {
            ids: vec![primary.id, alternate.id],
            ..Default::default()
        })
        .await
        .expect("ungrouped query must succeed");
    assert_eq!(ungrouped.total_record_count, 1);
    assert_eq!(ungrouped.items[0].id, primary.id);

    cleanup(&repository, &[primary.id, alternate.id]).await;
}

// Official GetItemList_GroupWithoutPrimary_FallsBackToMinId.
#[tokio::test]
async fn group_without_primary_falls_back_to_minimum_id() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database);
    let (first_id, second_id) = ordered_ids();
    let primary_id = Uuid::new_v4();
    let key = format!("fallback-group-{}", Uuid::new_v4().simple());
    let primary = create_item(
        &repository,
        primary_id,
        "Filtered Primary",
        Some(&key),
        None,
    )
    .await;
    let first = create_item(
        &repository,
        first_id,
        "Zulu First UUID",
        Some(&key),
        Some(primary.id),
    )
    .await;
    let second = create_item(
        &repository,
        second_id,
        "Alpha Second UUID",
        Some(&key),
        Some(primary.id),
    )
    .await;

    let grouped = repository
        .query(&grouped_query(&[first.id, second.id]))
        .await
        .expect("fallback version-group query must succeed");
    assert_eq!(grouped.items.len(), 1);
    assert_eq!(grouped.items[0].id, first.id);

    cleanup(&repository, &[primary.id, first.id, second.id]).await;
}

#[tokio::test]
async fn postgres_grouping_constraints_sorting_and_index_plan() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database.clone());
    let key_a = format!("multi-a-{}", Uuid::new_v4().simple());
    let key_b = format!("multi-b-{}", Uuid::new_v4().simple());
    let no_key_a = create_item(&repository, Uuid::new_v4(), "A no key", None, None).await;
    let no_key_b = create_item(&repository, Uuid::new_v4(), "B no key", None, None).await;
    let primary_a = create_item(
        &repository,
        Uuid::new_v4(),
        "Z group A primary",
        Some(&key_a),
        None,
    )
    .await;
    let alternate_a = create_item(
        &repository,
        Uuid::new_v4(),
        "C group A alternate",
        Some(&key_a),
        Some(primary_a.id),
    )
    .await;
    let primary_b = create_item(
        &repository,
        Uuid::new_v4(),
        "D group B primary",
        Some(&key_b),
        None,
    )
    .await;
    let alternate_b = create_item(
        &repository,
        Uuid::new_v4(),
        "E group B alternate",
        Some(&key_b),
        Some(primary_b.id),
    )
    .await;
    let ids = [
        no_key_a.id,
        no_key_b.id,
        primary_a.id,
        alternate_a.id,
        primary_b.id,
        alternate_b.id,
    ];

    let grouped = repository
        .query(&grouped_query(&ids))
        .await
        .expect("multi-group query must succeed");
    assert_eq!(grouped.total_record_count, 4);
    assert_eq!(
        grouped.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![no_key_a.id, no_key_b.id, primary_b.id, primary_a.id]
    );
    let paged = repository
        .query(&BaseItemQuery {
            ids: ids.to_vec(),
            group_versions_by_presentation_key: true,
            start_index: 1,
            limit: Some(2),
            ..Default::default()
        })
        .await
        .expect("grouped pagination must succeed");
    assert_eq!(paged.total_record_count, 4);
    assert_eq!(
        paged.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![no_key_b.id, primary_b.id]
    );

    assert_constraints(&repository).await;
    assert_delete_sets_null(&repository).await;
    assert_postgres_catalog_and_plan(&database).await;
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
            AddAlternateItemVersionsMigration
                .up(&schema)
                .await
                .expect("reapplying alternate-version DDL must succeed");
            AddAlternateItemVersionsMigration
                .up(&schema)
                .await
                .expect("alternate-version DDL must remain idempotent");
            database
        })
        .await
        .clone()
}

fn grouped_query(ids: &[Uuid]) -> BaseItemQuery {
    BaseItemQuery {
        ids: ids.to_vec(),
        group_versions_by_presentation_key: true,
        ..Default::default()
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    id: Uuid,
    sort_name: &str,
    presentation_key: Option<&str>,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, "Movie");
    item.name = Some(sort_name.to_owned());
    item.sort_name = Some(sort_name.to_owned());
    item.media_type = Some("Video".to_owned());
    item.presentation_unique_key = presentation_key.map(ToOwned::to_owned);
    item.primary_version_id = primary_version_id;
    repository
        .create(item)
        .await
        .expect("version item creation")
}

async fn assert_constraints(repository: &BaseItemRepository) {
    let self_id = Uuid::new_v4();
    let mut self_version = NewBaseItem::new(self_id, "Movie");
    self_version.primary_version_id = Some(self_id);
    assert!(matches!(
        repository.create(self_version).await,
        Err(BaseItemError::Database(_))
    ));

    let mut missing_primary = NewBaseItem::new(Uuid::new_v4(), "Movie");
    missing_primary.primary_version_id = Some(Uuid::new_v4());
    assert!(matches!(
        repository.create(missing_primary).await,
        Err(BaseItemError::Database(_))
    ));
}

async fn assert_delete_sets_null(repository: &BaseItemRepository) {
    let key = format!("delete-group-{}", Uuid::new_v4().simple());
    let primary = create_item(
        repository,
        Uuid::new_v4(),
        "Delete primary",
        Some(&key),
        None,
    )
    .await;
    let alternate = create_item(
        repository,
        Uuid::new_v4(),
        "Delete alternate",
        Some(&key),
        Some(primary.id),
    )
    .await;
    assert!(
        repository
            .delete(primary.id)
            .await
            .expect("primary deletion")
    );
    let alternate = repository
        .get(alternate.id)
        .await
        .expect("alternate lookup after primary deletion")
        .expect("alternate must survive primary deletion");
    assert_eq!(alternate.primary_version_id, None);
    assert!(
        repository
            .delete(alternate.id)
            .await
            .expect("alternate cleanup")
    );
}

async fn assert_postgres_catalog_and_plan(database: &DatabaseConnection) {
    let indexes = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname, indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' \
               AND indexname IN ('base_items_primary_version_id_idx', \
                                 'base_items_version_group_idx')"
                .to_owned(),
        ))
        .await
        .expect("version index catalog query must succeed");
    assert_eq!(indexes.len(), 2);
    let group_index = indexes
        .iter()
        .find(|row| {
            String::try_get(row, "", "indexname").expect("index name")
                == "base_items_version_group_idx"
        })
        .expect("version-group index must exist");
    let definition =
        String::try_get(group_index, "", "indexdef").expect("version-group index definition");
    assert!(definition.contains("primary_version_id IS NULL"));
    assert!(definition.contains("WHERE (presentation_unique_key IS NOT NULL)"));

    let transaction = database.begin().await.expect("version plan transaction");
    let suffix = Uuid::new_v4().simple().to_string();
    transaction
        .execute(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            r"INSERT INTO jellyfin.base_items
                   (id, item_type, name, sort_name, presentation_unique_key)
               SELECT md5($1 || '-primary-' || value::text)::uuid,
                      'Movie', 'Planner primary', 'Planner primary',
                      $1 || '-group-' || value::text
                 FROM generate_series(1, 2048) AS value",
            [suffix.clone().into()],
        ))
        .await
        .expect("planner primary insert must succeed");
    transaction
        .execute(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            r"INSERT INTO jellyfin.base_items
                   (id, item_type, name, sort_name, presentation_unique_key, primary_version_id)
               SELECT md5($1 || '-alternate-' || value::text)::uuid,
                      'Movie', 'Planner alternate', 'Planner alternate',
                      $1 || '-group-' || value::text,
                      md5($1 || '-primary-' || value::text)::uuid
                 FROM generate_series(1, 2048) AS value",
            [suffix.into()],
        ))
        .await
        .expect("planner alternate insert must succeed");
    transaction
        .execute_unprepared("ANALYZE jellyfin.base_items; SET LOCAL enable_seqscan = off")
        .await
        .expect("version planner statistics must refresh");
    let row = transaction
        .query_one(Statement::from_string(
            transaction.get_database_backend(),
            "EXPLAIN (FORMAT JSON) \
             SELECT DISTINCT ON (presentation_unique_key) id, sort_name \
             FROM jellyfin.base_items \
             WHERE presentation_unique_key IS NOT NULL \
             ORDER BY presentation_unique_key, (primary_version_id IS NULL) DESC, id"
                .to_owned(),
        ))
        .await
        .expect("version-group JSON explain must succeed")
        .expect("version-group JSON explain must return a row");
    let plan = Value::try_get(&row, "", "QUERY PLAN").expect("version JSON plan must decode");
    let serialized = plan.to_string();
    assert!(
        serialized.contains("base_items_version_group_idx"),
        "version grouping must use its index: {serialized}"
    );
    transaction.rollback().await.expect("version plan rollback");
}

async fn cleanup(repository: &BaseItemRepository, ids: &[Uuid]) {
    repository
        .delete_many(ids)
        .await
        .expect("version-group fixtures must clean up");
}

fn ordered_ids() -> (Uuid, Uuid) {
    let random_tail = Uuid::new_v4().as_u128() & 0x0000_0000_ffff_ffff_ffff_ffff_ffff_ffff;
    (
        Uuid::from_u128(0x1111_1111_0000_0000_0000_0000_0000_0000 | random_tail),
        Uuid::from_u128(0xeeee_eeee_0000_0000_0000_0000_0000_0000 | random_tail),
    )
}
