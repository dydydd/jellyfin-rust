use chrono::{Duration, Utc};
use jellyfin_data::{
    BaseItemQuery, BaseItemRepository, DatabaseConfig, NewBaseItem, NewUserData,
    UserDataRepository, entities::user,
};
use jellyfin_migration::OptimizeItemQueriesMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, EntityTrait, QueryResult, Statement, TransactionTrait,
    TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_item_query_vertical_slice() {
    let database = prepare_database().await;
    let fixture = Fixture::new(database.clone()).await;

    assert_item_filters_and_pagination(&fixture).await;
    assert_resume_order_and_deduplication(&fixture).await;
    assert_postgres_catalog(&database).await;
    assert_postgres_query_plans(&database, &fixture).await;

    fixture.cleanup().await;
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    OptimizeItemQueriesMigration
        .up(&schema)
        .await
        .expect("reapplying item-query indexes must succeed");
    OptimizeItemQueriesMigration
        .up(&schema)
        .await
        .expect("item-query indexes must remain idempotent");
    database
}

struct Fixture {
    database: DatabaseConnection,
    suffix: String,
    user_id: Uuid,
    container_id: Uuid,
    item_ids: [Uuid; 4],
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let user_id = Uuid::new_v4();
        insert_user(&database, user_id, &format!("ItemQuery-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let container = create_item(
            &items,
            "Folder",
            &format!("Container {suffix}"),
            Some(root.id),
            None,
            true,
        )
        .await;
        let first = create_item(
            &items,
            "Movie",
            &format!("A 100%_Ready {suffix}"),
            Some(container.id),
            Some("Video"),
            false,
        )
        .await;
        let second = create_item(
            &items,
            "Episode",
            &format!("B 100XYReady {suffix}"),
            Some(container.id),
            Some("Audio"),
            false,
        )
        .await;
        let folder = create_item(
            &items,
            "Folder",
            &format!("C {suffix}"),
            Some(container.id),
            None,
            true,
        )
        .await;
        let nested = create_item(
            &items,
            "Movie",
            &format!("D {suffix}"),
            Some(folder.id),
            Some("Video"),
            false,
        )
        .await;

        let user_data = UserDataRepository::new(database.clone());
        let now = Utc::now();
        insert_resume(
            &user_data,
            user_id,
            first.id,
            "older",
            100,
            now - Duration::hours(2),
        )
        .await;
        insert_resume(&user_data, user_id, first.id, "current", 200, now).await;
        insert_resume(
            &user_data,
            user_id,
            nested.id,
            "current",
            300,
            now - Duration::hours(1),
        )
        .await;
        insert_resume(&user_data, user_id, second.id, "current", 0, now).await;

        Self {
            database,
            suffix,
            user_id,
            container_id: container.id,
            item_ids: [first.id, second.id, folder.id, nested.id],
        }
    }

    async fn cleanup(self) {
        BaseItemRepository::new(self.database.clone())
            .delete(self.container_id)
            .await
            .expect("item-query subtree cleanup");
        user::Entity::delete_by_id(self.user_id)
            .exec(&self.database)
            .await
            .expect("item-query user cleanup");
    }
}

async fn assert_item_filters_and_pagination(fixture: &Fixture) {
    let repository = BaseItemRepository::new(fixture.database.clone());
    let page = repository
        .query(&BaseItemQuery {
            parent_id: Some(fixture.container_id),
            recursive: true,
            search_term: Some(fixture.suffix.to_uppercase()),
            start_index: 1,
            limit: Some(2),
            ..Default::default()
        })
        .await
        .expect("recursive paginated query");
    assert_eq!(page.total_record_count, 4);
    assert_eq!(page.start_index, 1);
    assert_eq!(page.items.len(), 2);

    let direct = repository
        .query(&BaseItemQuery {
            parent_id: Some(fixture.container_id),
            search_term: Some(fixture.suffix.to_uppercase()),
            ..Default::default()
        })
        .await
        .expect("direct child query");
    assert_eq!(direct.total_record_count, 3);

    let literal_wildcards = repository
        .query(&BaseItemQuery {
            parent_id: Some(fixture.container_id),
            recursive: true,
            search_term: Some("100%_ready".to_owned()),
            ..Default::default()
        })
        .await
        .expect("literal wildcard search");
    assert_eq!(literal_wildcards.total_record_count, 1);
    assert_eq!(literal_wildcards.items[0].id, fixture.item_ids[0]);

    let movies = repository
        .query(&BaseItemQuery {
            parent_id: Some(fixture.container_id),
            recursive: true,
            search_term: Some(fixture.suffix.to_uppercase()),
            include_item_types: vec!["Movie".to_owned()],
            media_types: vec!["Video".to_owned()],
            ..Default::default()
        })
        .await
        .expect("type and media query");
    assert_eq!(movies.total_record_count, 2);
    assert!(movies.items.iter().all(|item| item.item_type == "Movie"));

    let without_folders = repository
        .query(&BaseItemQuery {
            parent_id: Some(fixture.container_id),
            recursive: true,
            search_term: Some(fixture.suffix.to_uppercase()),
            exclude_item_types: vec!["Folder".to_owned()],
            ..Default::default()
        })
        .await
        .expect("excluded type query");
    assert_eq!(without_folders.total_record_count, 3);
}

async fn assert_resume_order_and_deduplication(fixture: &Fixture) {
    let page = BaseItemRepository::new(fixture.database.clone())
        .query_resumable(
            fixture.user_id,
            &BaseItemQuery {
                parent_id: Some(fixture.container_id),
                recursive: true,
                search_term: Some(fixture.suffix.to_uppercase()),
                is_virtual_item: Some(false),
                start_index: 1,
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .expect("database-side resume query");
    assert_eq!(page.total_record_count, 2);
    assert_eq!(page.start_index, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, fixture.item_ids[3]);

    let beyond_end = BaseItemRepository::new(fixture.database.clone())
        .query_resumable(
            fixture.user_id,
            &BaseItemQuery {
                parent_id: Some(fixture.container_id),
                recursive: true,
                search_term: Some(fixture.suffix.to_uppercase()),
                is_virtual_item: Some(false),
                start_index: 100,
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .expect("resume page beyond end");
    assert_eq!(beyond_end.total_record_count, 2);
    assert_eq!(beyond_end.start_index, 100);
    assert!(beyond_end.items.is_empty());
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let indexes = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname AS name, indexdef AS definition FROM pg_indexes \
             WHERE schemaname = 'jellyfin' \
               AND indexname IN ('base_items_clean_name_trgm_idx', 'user_data_resume_order_idx')"
                .to_owned(),
        ))
        .await
        .expect("item-query index catalog");
    let indexes = indexes
        .iter()
        .map(|row| {
            (
                String::try_get(row, "", "name").expect("index name"),
                String::try_get(row, "", "definition").expect("index definition"),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        "base_items_clean_name_trgm_idx",
        "user_data_resume_order_idx",
    ] {
        assert!(
            indexes.iter().any(|(name, _)| name == expected),
            "{expected}"
        );
    }
    let resume_definition = indexes
        .iter()
        .find(|(name, _)| name == "user_data_resume_order_idx")
        .map(|(_, definition)| definition.to_ascii_lowercase())
        .expect("resume index definition");
    assert!(resume_definition.contains("last_played_date desc nulls last, custom_data_key"));
    assert!(resume_definition.contains("include (playback_position_ticks)"));
    assert!(resume_definition.contains("where (playback_position_ticks > 0)"));
}

async fn assert_postgres_query_plans(database: &DatabaseConnection, fixture: &Fixture) {
    let transaction = database.begin().await.expect("planner transaction");
    transaction
        .execute(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r"INSERT INTO jellyfin.base_items
                   (id, item_type, parent_id, name, sort_name, media_type)
               SELECT md5($1 || '-item-' || value::text)::uuid, 'Movie', $2,
                      CASE WHEN value <= 8
                           THEN 'Rare Needle ' || $1 || ' ' || value::text
                           ELSE 'Planner Noise ' || value::text END,
                      CASE WHEN value <= 8
                           THEN 'Rare Needle ' || $1 || ' ' || value::text
                           ELSE 'Planner Noise ' || value::text END,
                      'Video'
                 FROM generate_series(1, 2048) AS value",
            [fixture.suffix.clone().into(), fixture.container_id.into()],
        ))
        .await
        .expect("seed item-query planner sample");
    transaction
        .execute(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r"INSERT INTO jellyfin.user_data
                   (item_id, user_id, custom_data_key, playback_position_ticks, last_played_date)
               SELECT md5($1 || '-item-' || value::text)::uuid, $2,
                      'key-' || value::text, value,
                      clock_timestamp() - (value || ' seconds')::interval
                 FROM generate_series(1, 2048) AS value",
            [fixture.suffix.clone().into(), fixture.user_id.into()],
        ))
        .await
        .expect("seed resume planner sample");
    transaction
        .execute_unprepared("ANALYZE jellyfin.base_items; ANALYZE jellyfin.ancestor_ids; ANALYZE jellyfin.user_data")
        .await
        .expect("analyze item-query tables");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");

    assert_item_query_plans(&transaction, fixture).await;
    assert_resume_query_plan(&transaction, fixture).await;
    transaction.rollback().await.expect("planner rollback");
}

async fn assert_item_query_plans(transaction: &sea_orm::DatabaseTransaction, fixture: &Fixture) {
    let recursive_plan = explain(
        transaction,
        "EXPLAIN (FORMAT TEXT) SELECT item.* FROM jellyfin.base_items AS item \
         WHERE item.item_type <> 'PLACEHOLDER' \
           AND item.id IN (SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
                           WHERE closure.parent_item_id = $1) \
         ORDER BY item.sort_name, item.id LIMIT 25",
        [fixture.container_id.into()],
    )
    .await;
    assert!(
        recursive_plan.contains("ancestor_ids_parent_depth_idx"),
        "expected hierarchy index in:\n{recursive_plan}"
    );

    let search_plan = explain(
        transaction,
        "EXPLAIN (FORMAT TEXT) SELECT item.* FROM jellyfin.base_items AS item \
         WHERE item.item_type <> 'PLACEHOLDER' AND item.clean_name ILIKE $1 \
         ORDER BY item.sort_name, item.id LIMIT 25",
        [format!("%rARE nEEDLE {}%", fixture.suffix.to_uppercase()).into()],
    )
    .await;
    assert!(
        search_plan.contains("base_items_clean_name_trgm_idx"),
        "expected trigram index in:\n{search_plan}"
    );
}

async fn assert_resume_query_plan(transaction: &sea_orm::DatabaseTransaction, fixture: &Fixture) {
    let resume_plan = explain(
        transaction,
        "EXPLAIN (FORMAT TEXT) WITH resumable AS ( \
             SELECT DISTINCT ON (item_id) item_id, last_played_date \
             FROM jellyfin.user_data \
             WHERE user_id = $1 AND playback_position_ticks > 0 \
             ORDER BY item_id, last_played_date DESC NULLS LAST, custom_data_key \
         ), filtered AS ( \
             SELECT item.*, resumable.last_played_date AS resume_last_played_date \
             FROM resumable \
             JOIN jellyfin.base_items AS item ON item.id = resumable.item_id \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.id IN (SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
                               WHERE closure.parent_item_id = $2) \
               AND item.clean_name ILIKE $3 \
               AND item.is_virtual_item = $4 \
         ) SELECT id FROM filtered \
           ORDER BY resume_last_played_date DESC NULLS LAST, id OFFSET $5 LIMIT $6",
        [
            fixture.user_id.into(),
            fixture.container_id.into(),
            format!("%rARE nEEDLE {}%", fixture.suffix.to_uppercase()).into(),
            false.into(),
            0_i64.into(),
            25_i64.into(),
        ],
    )
    .await;
    assert!(
        resume_plan.contains("user_data_resume_order_idx")
            || resume_plan.contains("user_data_resume_idx"),
        "expected a dedicated resume partial index in:\n{resume_plan}"
    );
}

async fn explain<const N: usize>(
    transaction: &sea_orm::DatabaseTransaction,
    sql: &str,
    values: [sea_orm::Value; N],
) -> String {
    transaction
        .query_all(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            sql,
            values,
        ))
        .await
        .expect("EXPLAIN item query")
        .iter()
        .map(explain_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn explain_line(row: &QueryResult) -> String {
    String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line")
}

async fn insert_user(database: &DatabaseConnection, user_id: Uuid, username: &str) {
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.users (id, username, normalized_username) VALUES ($1, $2, $3)",
            [
                user_id.into(),
                username.into(),
                username.to_uppercase().into(),
            ],
        ))
        .await
        .expect("item-query user insert");
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    media_type: Option<&str>,
    is_folder: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.media_type = media_type.map(ToOwned::to_owned);
    item.is_folder = is_folder;
    repository
        .create(item)
        .await
        .expect("item-query item insert")
}

async fn insert_resume(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    key: &str,
    position: i64,
    last_played_date: chrono::DateTime<Utc>,
) {
    let mut data = NewUserData::new(item_id, user_id, key);
    data.playback_position_ticks = position;
    data.last_played_date = Some(last_played_date);
    repository.upsert(data).await.expect("resume row insert");
}
