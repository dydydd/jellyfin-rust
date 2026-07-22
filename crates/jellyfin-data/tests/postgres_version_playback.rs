use chrono::{TimeZone, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemOrder, BaseItemQuery, BaseItemRepository, DatabaseConfig, NewBaseItem,
    NewUserData, UserDataRepository,
};
use jellyfin_migration::OptimizeVersionPlaybackMigration;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, TransactionTrait, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::Value;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static TEST_LOCK: Mutex<()> = Mutex::const_new(());

// Official ResumeFilter_VersionProgress_SurfacesPlayedVersion.
#[tokio::test]
async fn resume_surfaces_played_alternate_and_not_resumable_keeps_other_primary() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let user_id = insert_user(&database).await;
    let items = BaseItemRepository::new(database.clone());
    let user_data = UserDataRepository::new(database.clone());
    let primary = create_item(&items, Uuid::new_v4(), "A primary", None).await;
    let alternate = create_item(
        &items,
        Uuid::new_v4(),
        "A played alternate",
        Some(primary.id),
    )
    .await;
    let other = create_item(&items, Uuid::new_v4(), "B other primary", None).await;
    let played_at = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    insert_progress(
        &user_data,
        user_id,
        alternate.id,
        "alternate",
        1_000,
        played_at,
    )
    .await;

    let query = BaseItemQuery {
        ids: vec![primary.id, alternate.id, other.id],
        ..Default::default()
    };
    let resumable = items
        .query_resumable(user_id, &query)
        .await
        .expect("version-aware resume query");
    assert_eq!(resumable.total_record_count, 1);
    assert_eq!(resumable.items[0].id, alternate.id);

    let not_resumable = items
        .query(&BaseItemQuery {
            user_id: Some(user_id),
            is_resumable: Some(false),
            ..query.clone()
        })
        .await
        .expect("version-aware not-resumable query");
    assert_eq!(not_resumable.total_record_count, 1);
    assert_eq!(not_resumable.items[0].id, other.id);

    cleanup(
        &database,
        &items,
        user_id,
        &[primary.id, alternate.id, other.id],
    )
    .await;
}

// Official ResumeFilter_TiedLastPlayedDate_KeepsSingleVersion.
#[tokio::test]
async fn resume_ties_keep_lowest_uuid_with_stable_count_and_pagination() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let user_id = insert_user(&database).await;
    let items = BaseItemRepository::new(database.clone());
    let user_data = UserDataRepository::new(database.clone());
    let primary_a = create_item(&items, Uuid::new_v4(), "Group A primary", None).await;
    let (version_a_low_id, version_a_high_id) = ordered_ids();
    let version_a_low =
        create_item(&items, version_a_low_id, "Group A low", Some(primary_a.id)).await;
    let version_a_high = create_item(
        &items,
        version_a_high_id,
        "Group A high",
        Some(primary_a.id),
    )
    .await;
    let primary_b = create_item(&items, Uuid::new_v4(), "Group B primary", None).await;
    let version_b = create_item(
        &items,
        Uuid::new_v4(),
        "Group B version",
        Some(primary_b.id),
    )
    .await;
    let tied = Utc.with_ymd_and_hms(2026, 2, 3, 0, 0, 0).unwrap();
    insert_progress(&user_data, user_id, version_a_low.id, "low", 100, tied).await;
    insert_progress(&user_data, user_id, version_a_high.id, "high", 200, tied).await;
    insert_progress(&user_data, user_id, version_b.id, "old-key", 300, tied).await;
    insert_progress(&user_data, user_id, version_b.id, "new-key", 400, tied).await;

    let ids = [
        primary_a.id,
        version_a_low.id,
        version_a_high.id,
        primary_b.id,
        version_b.id,
    ];
    let query = BaseItemQuery {
        ids: ids.to_vec(),
        ..Default::default()
    };
    let page = items
        .query_resumable(user_id, &query)
        .await
        .expect("tied version resume query");
    let mut expected = vec![version_a_low.id, version_b.id];
    expected.sort_unstable();
    assert_eq!(page.total_record_count, 2);
    assert_eq!(
        page.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        expected
    );

    let paged = items
        .query_resumable(
            user_id,
            &BaseItemQuery {
                start_index: 1,
                limit: Some(1),
                ..query
            },
        )
        .await
        .expect("tied version resume page");
    assert_eq!(paged.total_record_count, 2);
    assert_eq!(paged.items.len(), 1);
    assert_eq!(paged.items[0].id, expected[1]);

    cleanup(&database, &items, user_id, &ids).await;
}

// Official DatePlayedOrdering_VersionProgress_SortsPrimaryByVersionDate.
#[tokio::test]
async fn date_played_rolls_version_dates_up_to_primary_and_preserves_default_query() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let user_id = insert_user(&database).await;
    let items = BaseItemRepository::new(database.clone());
    let user_data = UserDataRepository::new(database.clone());
    let primary = create_item(&items, Uuid::new_v4(), "Z primary", None).await;
    let alternate = create_item(&items, Uuid::new_v4(), "A alternate", Some(primary.id)).await;
    let (tied_low_id, tied_high_id) = ordered_ids();
    let tied_low = create_item(&items, tied_low_id, "Tied low", None).await;
    let tied_high = create_item(&items, tied_high_id, "Tied high", None).await;
    let unplayed = create_item(&items, Uuid::new_v4(), "Unplayed", None).await;
    let latest = Utc.with_ymd_and_hms(2026, 3, 4, 0, 0, 0).unwrap();
    let tied = Utc.with_ymd_and_hms(2026, 3, 3, 0, 0, 0).unwrap();
    insert_progress(&user_data, user_id, alternate.id, "alternate", 100, latest).await;
    insert_progress(&user_data, user_id, tied_low.id, "low", 0, tied).await;
    insert_progress(&user_data, user_id, tied_high.id, "high", 0, tied).await;

    let ids = [
        primary.id,
        alternate.id,
        tied_low.id,
        tied_high.id,
        unplayed.id,
    ];
    let ordered = items
        .query(&BaseItemQuery {
            ids: ids.to_vec(),
            user_id: Some(user_id),
            order: BaseItemOrder::DatePlayedDescending,
            ..Default::default()
        })
        .await
        .expect("DatePlayed version roll-up query");
    assert_eq!(ordered.total_record_count, 4);
    assert_eq!(
        ordered.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![primary.id, tied_low.id, tied_high.id, unplayed.id]
    );

    let paged = items
        .query(&BaseItemQuery {
            ids: ids.to_vec(),
            user_id: Some(user_id),
            order: BaseItemOrder::DatePlayedDescending,
            start_index: 1,
            limit: Some(2),
            ..Default::default()
        })
        .await
        .expect("DatePlayed version roll-up page");
    assert_eq!(paged.total_record_count, 4);
    assert_eq!(
        paged.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![tied_low.id, tied_high.id]
    );

    let default_query = items
        .query(&BaseItemQuery {
            ids: vec![primary.id, alternate.id],
            ..Default::default()
        })
        .await
        .expect("default query remains ungrouped");
    assert_eq!(default_query.total_record_count, 2);
    assert_eq!(default_query.items[0].id, alternate.id);
    assert_eq!(default_query.items[1].id, primary.id);

    let missing_user = items
        .query(&BaseItemQuery {
            ids: vec![primary.id],
            order: BaseItemOrder::DatePlayedDescending,
            ..Default::default()
        })
        .await;
    assert!(matches!(missing_user, Err(BaseItemError::UserRequired)));

    assert_postgres_catalog_and_plans(&database, user_id).await;
    cleanup(&database, &items, user_id, &ids).await;
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
            OptimizeVersionPlaybackMigration
                .up(&schema)
                .await
                .expect("reapplying version-playback DDL must succeed");
            OptimizeVersionPlaybackMigration
                .up(&schema)
                .await
                .expect("version-playback DDL must remain idempotent");
            database
        })
        .await
        .clone()
}

async fn insert_user(database: &DatabaseConnection) -> Uuid {
    let user_id = Uuid::new_v4();
    let username = format!("VersionPlayback-{}", user_id.simple());
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.users (id, username, normalized_username) VALUES ($1, $2, $3)",
            [
                user_id.into(),
                username.clone().into(),
                username.to_uppercase().into(),
            ],
        ))
        .await
        .expect("version-playback user insertion");
    user_id
}

async fn create_item(
    repository: &BaseItemRepository,
    id: Uuid,
    sort_name: &str,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, "Movie");
    item.name = Some(sort_name.to_owned());
    item.sort_name = Some(sort_name.to_owned());
    item.media_type = Some("Video".to_owned());
    item.primary_version_id = primary_version_id;
    repository
        .create(item)
        .await
        .expect("version-playback item creation")
}

async fn insert_progress(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    key: &str,
    position: i64,
    last_played_date: chrono::DateTime<Utc>,
) {
    let mut progress = NewUserData::new(item_id, user_id, key);
    progress.playback_position_ticks = position;
    progress.last_played_date = Some(last_played_date);
    repository
        .upsert(progress)
        .await
        .expect("version progress insertion");
}

async fn assert_postgres_catalog_and_plans(database: &DatabaseConnection, user_id: Uuid) {
    assert_version_date_index(database).await;
    let transaction = database.begin().await.expect("planner transaction");
    seed_planner_data(&transaction, user_id).await;
    transaction
        .execute_unprepared(
            "ANALYZE jellyfin.base_items; ANALYZE jellyfin.user_data; \
             SET LOCAL enable_seqscan = off; SET LOCAL enable_bitmapscan = off",
        )
        .await
        .expect("planner statistics refresh");
    assert_version_query_plans(&transaction, user_id).await;
    transaction.rollback().await.expect("planner rollback");
}

async fn assert_version_date_index(database: &DatabaseConnection) {
    let row = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' \
               AND indexname = 'user_data_version_date_played_idx'"
                .to_owned(),
        ))
        .await
        .expect("version date index catalog query")
        .expect("version date index must exist");
    let definition = String::try_get(&row, "", "indexdef").expect("version date index definition");
    assert!(definition.contains("user_id, item_id, last_played_date DESC NULLS LAST"));
    assert!(definition.contains("WHERE (last_played_date IS NOT NULL)"));
}

async fn seed_planner_data(transaction: &sea_orm::DatabaseTransaction, user_id: Uuid) {
    let suffix = Uuid::new_v4().simple().to_string();
    transaction
        .execute(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            r"INSERT INTO jellyfin.base_items (id, item_type, name, sort_name)
               SELECT md5($1 || '-primary-' || value::text)::uuid,
                      'Movie', 'Planner primary', 'Planner primary'
                 FROM generate_series(1, 2048) AS value",
            [suffix.clone().into()],
        ))
        .await
        .expect("planner primary insertion");
    transaction
        .execute(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            r"INSERT INTO jellyfin.base_items
                   (id, item_type, name, sort_name, primary_version_id)
               SELECT md5($1 || '-alternate-' || value::text)::uuid,
                      'Movie', 'Planner alternate', 'Planner alternate',
                      md5($1 || '-primary-' || value::text)::uuid
                 FROM generate_series(1, 2048) AS value",
            [suffix.clone().into()],
        ))
        .await
        .expect("planner alternate insertion");
    transaction
        .execute(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            r"INSERT INTO jellyfin.user_data
                   (item_id, user_id, custom_data_key, playback_position_ticks, last_played_date)
               SELECT md5($1 || '-alternate-' || value::text)::uuid, $2,
                      'planner-' || value::text, value,
                      clock_timestamp() - (value || ' seconds')::interval
                 FROM generate_series(1, 2048) AS value",
            [suffix.into(), user_id.into()],
        ))
        .await
        .expect("planner progress insertion");
}

async fn assert_version_query_plans(transaction: &sea_orm::DatabaseTransaction, user_id: Uuid) {
    let resume_plan = explain_json(
        transaction,
        r"EXPLAIN (FORMAT JSON)
           WITH progress_by_item AS (
               SELECT item_id, MAX(last_played_date) AS resume_last_played_date
               FROM jellyfin.user_data
               WHERE user_id = $1 AND playback_position_ticks > 0
               GROUP BY item_id
           ), resume_versions AS (
               SELECT DISTINCT ON (COALESCE(item.primary_version_id, item.id))
                      item.id, progress.resume_last_played_date
               FROM progress_by_item AS progress
               JOIN jellyfin.base_items AS item ON item.id = progress.item_id
               ORDER BY COALESCE(item.primary_version_id, item.id),
                        progress.resume_last_played_date DESC NULLS LAST, item.id
           )
           SELECT id FROM resume_versions
           ORDER BY resume_last_played_date DESC NULLS LAST, id",
        user_id,
    )
    .await;
    assert!(
        resume_plan.contains("user_data_resume_idx"),
        "resume query must use its existing partial index: {resume_plan}"
    );

    let date_plan = explain_json(
        transaction,
        r"EXPLAIN (FORMAT JSON)
           WITH version_dates AS (
               SELECT COALESCE(item.primary_version_id, item.id) AS primary_id,
                      MAX(progress.last_played_date) AS date_played
               FROM jellyfin.user_data AS progress
               JOIN jellyfin.base_items AS item ON item.id = progress.item_id
               WHERE progress.user_id = $1 AND progress.last_played_date IS NOT NULL
               GROUP BY COALESCE(item.primary_version_id, item.id)
           )
           SELECT item.id
           FROM jellyfin.base_items AS item
           LEFT JOIN version_dates ON version_dates.primary_id = item.id
           WHERE item.primary_version_id IS NULL
           ORDER BY version_dates.date_played DESC NULLS LAST, item.id",
        user_id,
    )
    .await;
    assert!(
        date_plan.contains("user_data_version_date_played_idx"),
        "DatePlayed query must use its partial covering index: {date_plan}"
    );
}

async fn explain_json(
    transaction: &sea_orm::DatabaseTransaction,
    sql: &str,
    user_id: Uuid,
) -> String {
    let row = transaction
        .query_one(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            sql,
            [user_id.into()],
        ))
        .await
        .expect("version query JSON explain")
        .expect("version query JSON explain row");
    Value::try_get(&row, "", "QUERY PLAN")
        .expect("version query JSON plan")
        .to_string()
}

async fn cleanup(
    database: &DatabaseConnection,
    repository: &BaseItemRepository,
    user_id: Uuid,
    item_ids: &[Uuid],
) {
    repository
        .delete_many(item_ids)
        .await
        .expect("version-playback item cleanup");
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "DELETE FROM jellyfin.users WHERE id = $1",
            [user_id.into()],
        ))
        .await
        .expect("version-playback user cleanup");
}

fn ordered_ids() -> (Uuid, Uuid) {
    let tail = Uuid::new_v4().as_u128() & 0x0000_0000_ffff_ffff_ffff_ffff_ffff_ffff;
    (
        Uuid::from_u128(0x1111_1111_0000_0000_0000_0000_0000_0000 | tail),
        Uuid::from_u128(0xeeee_eeee_0000_0000_0000_0000_0000_0000 | tail),
    )
}
