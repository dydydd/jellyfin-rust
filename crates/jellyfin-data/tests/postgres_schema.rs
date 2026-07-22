use jellyfin_migration::CreateUsersMigration;
use sea_orm::{ConnectionTrait, Statement, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};

#[tokio::test]
async fn postgres_schema_installs_specialized_indexes() {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let (first_migration, second_migration) = tokio::join!(
        jellyfin_data::migrate(&database),
        jellyfin_data::migrate(&database)
    );
    first_migration.expect("first PostgreSQL migration must succeed");
    second_migration.expect("concurrent PostgreSQL migration must succeed");

    let schema = SchemaManager::new(&database);
    CreateUsersMigration
        .up(&schema)
        .await
        .expect("reapplying the PostgreSQL DDL must succeed");
    CreateUsersMigration
        .up(&schema)
        .await
        .expect("the PostgreSQL DDL must remain idempotent");

    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'users'"
                .to_owned(),
        ))
        .await
        .expect("PostgreSQL catalog query must succeed");
    let names: Vec<String> = rows
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name must be text"))
        .collect();

    assert!(names.iter().any(|name| name == "users_active_idx"));
    assert!(names.iter().any(|name| name == "users_username_trgm_idx"));
    assert!(names.iter().any(|name| name == "users_policy_gin_idx"));
    assert!(
        names
            .iter()
            .any(|name| name == "users_normalized_username_key")
    );
}
