use jellyfin_data::{
    DatabaseConfig, ServerConfigurationRepository, ServerConfigurationStoreError,
    StartupConfigurationUpdate,
};
use jellyfin_migration::CreateServerConfigurationMigration;
use sea_orm::{ConnectionTrait, EntityTrait, PaginatorTrait, Statement, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_startup_data_";

#[tokio::test]
async fn postgres_server_configuration_is_singleton_atomic_and_versioned() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_server_configuration(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_server_configuration(database_name: &str) {
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

    let schema = SchemaManager::new(&database);
    CreateServerConfigurationMigration
        .up(&schema)
        .await
        .expect("reapplying server-configuration DDL must succeed");
    CreateServerConfigurationMigration
        .up(&schema)
        .await
        .expect("server-configuration DDL must remain idempotent");
    assert_singleton_schema(&database).await;

    let first = ServerConfigurationRepository::new(database.clone());
    let second = ServerConfigurationRepository::new(database.clone());
    let seeded = first.load().await.expect("seeded singleton");
    assert_eq!(seeded.id, 1);
    assert_eq!(seeded.server_name, "Jellyfin");
    assert!(!seeded.is_startup_wizard_completed);
    assert_eq!(seeded.row_version, 1);

    let updated = first
        .update_startup_configuration(configuration("First update"))
        .await
        .expect("startup configuration update");
    assert_eq!(updated.server_name, "First update");
    assert!(updated.row_version > seeded.row_version);
    assert_eq!(updated.created_at, seeded.created_at);
    assert_eq!(second.load().await.unwrap(), updated);

    let (configuration_result, completion_result) = tokio::join!(
        first.update_startup_configuration(configuration("Concurrent update")),
        second.complete_startup(),
    );
    configuration_result.expect("concurrent configuration update");
    completion_result.expect("concurrent completion update");
    let concurrent = first.load().await.expect("post-concurrency load");
    assert_eq!(concurrent.server_name, "Concurrent update");
    assert_eq!(concurrent.ui_culture, "nl-BE");
    assert!(concurrent.is_startup_wizard_completed);
    assert!(concurrent.row_version >= updated.row_version + 2);

    let invalid_insert = database
        .execute_unprepared(
            "INSERT INTO jellyfin.server_configuration (id, server_name) VALUES (2, 'invalid')",
        )
        .await;
    assert!(invalid_insert.is_err(), "singleton check must reject id 2");
    assert_eq!(
        jellyfin_data::entities::server_configuration::Entity::find()
            .count(&database)
            .await
            .expect("configuration count"),
        1
    );

    jellyfin_data::entities::server_configuration::Entity::delete_by_id(1_i16)
        .exec(&database)
        .await
        .expect("singleton deletion fixture");
    assert!(matches!(
        first.load().await,
        Err(ServerConfigurationStoreError::MissingSingleton)
    ));
    assert!(matches!(
        first
            .update_startup_configuration(configuration("Missing"))
            .await,
        Err(ServerConfigurationStoreError::MissingSingleton)
    ));
    assert!(matches!(
        first.complete_startup().await,
        Err(ServerConfigurationStoreError::MissingSingleton)
    ));

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

fn configuration(server_name: &str) -> StartupConfigurationUpdate {
    StartupConfigurationUpdate {
        server_name: server_name.to_owned(),
        ui_culture: "nl-BE".to_owned(),
        metadata_country_code: "be".to_owned(),
        preferred_metadata_language: "nl".to_owned(),
    }
}

async fn assert_singleton_schema(database: &sea_orm::DatabaseConnection) {
    let indexes = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'server_configuration' \
             ORDER BY indexname"
                .to_owned(),
        ))
        .await
        .expect("server-configuration index catalog query")
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name"))
        .collect::<Vec<_>>();
    assert_eq!(indexes, ["server_configuration_pkey"]);

    let row = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT count(*)::bigint AS count FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.server_configuration'::regclass \
               AND conname = 'server_configuration_singleton'"
                .to_owned(),
        ))
        .await
        .expect("singleton constraint catalog query")
        .expect("constraint count row");
    assert_eq!(i64::try_get(&row, "", "count").unwrap(), 1);
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
