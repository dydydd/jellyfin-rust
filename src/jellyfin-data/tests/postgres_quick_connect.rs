use chrono::{Duration, Utc};
use jellyfin_data::{
    DatabaseConfig, NewQuickConnectRequest, QuickConnectRepository, QuickConnectStoreError,
    entities::quick_connect,
};
use jellyfin_migration::CreateQuickConnectMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement,
    TransactionTrait, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_quick_connect_schema_and_uniqueness() {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreateQuickConnectMigration
        .up(&schema)
        .await
        .expect("reapplying Quick Connect DDL must succeed");
    CreateQuickConnectMigration
        .up(&schema)
        .await
        .expect("Quick Connect DDL must remain idempotent");

    let index_rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' \
               AND tablename = 'quick_connect_requests'"
                .to_owned(),
        ))
        .await
        .expect("PostgreSQL catalog query must succeed");
    let indexes = index_rows
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name must be text"))
        .collect::<Vec<_>>();
    for expected in [
        "quick_connect_code_key",
        "quick_connect_secret_key",
        "quick_connect_expires_at_idx",
        "quick_connect_authorized_device_id_idx",
    ] {
        assert!(indexes.iter().any(|index| index == expected));
    }

    let suffix = Uuid::new_v4().simple().to_string().to_uppercase();
    let secret = format!("{suffix}{suffix}");
    let repository = QuickConnectRepository::new(database.clone());
    let now = Utc::now();
    let request = NewQuickConnectRequest {
        code: unused_code(&database).await,
        secret: secret.clone(),
        device_id: format!("schema-{suffix}"),
        device_name: "Schema Device".to_owned(),
        app_name: "Schema Client".to_owned(),
        app_version: "1.0".to_owned(),
        created_at: now,
        expires_at: now + Duration::minutes(10),
    };
    let inserted = repository
        .create(request.clone())
        .await
        .expect("Quick Connect request must insert");
    let conflict = repository
        .create(NewQuickConnectRequest {
            code: unused_code(&database).await,
            ..request
        })
        .await
        .expect_err("duplicate secret must be rejected");
    assert!(matches!(conflict, QuickConnectStoreError::Conflict));

    assert_expiry_index_plan(&database).await;

    quick_connect::Entity::delete_many()
        .filter(quick_connect::Column::Secret.eq(&inserted.secret))
        .exec(&database)
        .await
        .expect("test request cleanup must succeed");
}

async fn unused_code(database: &DatabaseConnection) -> String {
    loop {
        let candidate = (100_000 + Uuid::new_v4().as_u128() % 900_000).to_string();
        if quick_connect::Entity::find()
            .filter(quick_connect::Column::Code.eq(&candidate))
            .one(database)
            .await
            .expect("unused Quick Connect code lookup must succeed")
            .is_none()
        {
            return candidate;
        }
    }
}

async fn assert_expiry_index_plan(database: &DatabaseConnection) {
    let transaction = database.begin().await.expect("explain transaction");
    transaction
        .execute_unprepared(
            "ANALYZE jellyfin.quick_connect_requests; SET LOCAL enable_seqscan = off",
        )
        .await
        .expect("prepare Quick Connect explain plan");
    let row = transaction
        .query_one(Statement::from_string(
            transaction.get_database_backend(),
            "EXPLAIN (FORMAT JSON) DELETE FROM jellyfin.quick_connect_requests \
             WHERE expires_at <= CURRENT_TIMESTAMP"
                .to_owned(),
        ))
        .await
        .expect("expiry explain query must succeed")
        .expect("expiry explain query must return one row");
    let plan = serde_json::Value::try_get(&row, "", "QUERY PLAN")
        .expect("PostgreSQL JSON explain plan must decode");
    let serialized = plan.to_string();
    assert!(
        serialized.contains("quick_connect_expires_at_idx"),
        "expiry cleanup must use quick_connect_expires_at_idx: {serialized}"
    );
    transaction.rollback().await.expect("explain rollback");
}
