use chrono::{DateTime, Duration, Utc};
use jellyfin_data::{
    ActivityLogQuery, ActivityLogRepository, ActivityLogSortBy, DatabaseConfig, NewActivityLog,
    SortDirection,
    entities::activity_log::{self, LogSeverity},
};
use jellyfin_migration::CreateActivityLogsMigration;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, Set, Statement, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_activity_log_vertical_slice() {
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let marker = format!("Activity-{suffix}");
    let username = format!("ActivityUser-{suffix}");
    let user_id = Uuid::new_v4();
    insert_user(&database, user_id, &username).await;

    let repository = ActivityLogRepository::new(database.clone());
    let fixtures = insert_activities(&repository, &marker, user_id).await;
    assert_queries(&repository, &fixtures, &marker, &username).await;
    assert_update_and_cleanup(&database, &repository, fixtures, user_id).await;
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    CreateActivityLogsMigration
        .up(&schema)
        .await
        .expect("reapplying activity log DDL must succeed");
    CreateActivityLogsMigration
        .up(&schema)
        .await
        .expect("activity log DDL must remain idempotent");
    assert_postgres_indexes(&database).await;
    database
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
        .expect("activity log test user must be inserted");
}

struct ActivityFixtures {
    base_date: DateTime<Utc>,
    item_id: Uuid,
    oldest: activity_log::Model,
    middle: activity_log::Model,
    newest: activity_log::Model,
}

async fn insert_activities(
    repository: &ActivityLogRepository,
    marker: &str,
    user_id: Uuid,
) -> ActivityFixtures {
    // Dates this old make the retention test safe for a shared local database.
    let base_date = Utc::now() - Duration::days(365 * 200);
    let item_id = Uuid::new_v4();

    let mut oldest = NewActivityLog::new(format!("Oldest {marker}"), "SessionStarted", user_id);
    oldest.overview = Some("Client connected".to_owned());
    oldest.item_id = Some(item_id.simple().to_string());
    oldest.date_created = Some(base_date);
    oldest.log_severity = LogSeverity::Warning;
    let oldest = repository
        .create(oldest)
        .await
        .expect("oldest activity must be created");

    let mut middle = NewActivityLog::new(format!("Middle {marker}"), "PlaybackStart", Uuid::nil());
    middle.short_overview = Some("Started playback".to_owned());
    middle.date_created = Some(base_date + Duration::days(1));
    middle.log_severity = LogSeverity::Debug;
    let middle = repository
        .create(middle)
        .await
        .expect("middle activity must be created");

    let mut newest = NewActivityLog::new(format!("Newest {marker}"), "SessionEnded", user_id);
    newest.date_created = Some(base_date + Duration::days(2));
    let newest = repository
        .create(newest)
        .await
        .expect("newest activity must be created");

    ActivityFixtures {
        base_date,
        item_id,
        oldest,
        middle,
        newest,
    }
}

async fn assert_queries(
    repository: &ActivityLogRepository,
    fixtures: &ActivityFixtures,
    marker: &str,
    username: &str,
) {
    let page = repository
        .query(&ActivityLogQuery {
            name: Some(marker.to_lowercase()),
            ..Default::default()
        })
        .await
        .expect("case-insensitive activity query must succeed");
    assert_eq!(page.total_record_count, 3);
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.items[0].id, fixtures.newest.id);
    assert_eq!(page.items[1].id, fixtures.middle.id);
    assert_eq!(page.items[2].id, fixtures.oldest.id);

    let page = repository
        .query(&ActivityLogQuery {
            skip: Some(1),
            limit: Some(1),
            name: Some(marker.to_owned()),
            order_by: vec![(ActivityLogSortBy::DateCreated, SortDirection::Descending)],
            ..Default::default()
        })
        .await
        .expect("paged activity query must succeed");
    assert_eq!(page.start_index, Some(1));
    assert_eq!(page.total_record_count, 3);
    assert_eq!(page.items[0].id, fixtures.middle.id);

    let page = repository
        .query(&ActivityLogQuery {
            name: Some(marker.to_owned()),
            has_user_id: Some(false),
            ..Default::default()
        })
        .await
        .expect("user-presence activity query must succeed");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, fixtures.middle.id);

    let page = repository
        .query(&ActivityLogQuery {
            name: Some(marker.to_owned()),
            username: Some(username.to_lowercase()),
            severity: Some(LogSeverity::Warning),
            item_id: Some(fixtures.item_id),
            min_date: Some(fixtures.base_date - Duration::seconds(1)),
            max_date: Some(fixtures.base_date + Duration::seconds(1)),
            ..Default::default()
        })
        .await
        .expect("combined activity filters must succeed");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, fixtures.oldest.id);
}

async fn assert_update_and_cleanup(
    database: &DatabaseConnection,
    repository: &ActivityLogRepository,
    fixtures: ActivityFixtures,
    user_id: Uuid,
) {
    let mut updated = fixtures.newest.clone().into_active_model();
    updated.overview = Set(Some("Session closed".to_owned()));
    let updated = updated
        .update(database)
        .await
        .expect("activity update must succeed");
    assert_eq!(updated.row_version, fixtures.newest.row_version + 1);

    let deleted = repository
        .clean(fixtures.base_date)
        .await
        .expect("activity retention cleanup must succeed");
    assert_eq!(deleted, 1);

    activity_log::Entity::delete_many()
        .filter(activity_log::Column::Id.is_in([fixtures.middle.id, fixtures.newest.id]))
        .exec(database)
        .await
        .expect("activity test rows must be cleaned up");
    jellyfin_data::entities::user::Entity::delete_by_id(user_id)
        .exec(database)
        .await
        .expect("activity test user must be cleaned up");
}

async fn assert_postgres_indexes(database: &sea_orm::DatabaseConnection) {
    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname, indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'activity_logs'"
                .to_owned(),
        ))
        .await
        .expect("activity log index catalog query must succeed");
    let indexes: Vec<(String, String)> = rows
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "indexname").expect("index name must be text"),
                String::try_get(&row, "", "indexdef").expect("index definition must be text"),
            )
        })
        .collect();

    assert!(indexes.iter().any(|(name, definition)| {
        name == "activity_logs_date_created_brin_idx"
            && definition.to_ascii_lowercase().contains("using brin")
    }));
    assert!(indexes.iter().any(|(name, definition)| {
        name == "activity_logs_item_id_idx" && definition.contains("WHERE (item_id IS NOT NULL)")
    }));
    assert!(
        indexes
            .iter()
            .all(|(_, definition)| !definition.to_ascii_lowercase().contains("using gin")),
        "activity logs must not gain an ungrounded GIN index"
    );
}
