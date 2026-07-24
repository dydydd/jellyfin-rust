use jellyfin_migration::{
    AddBaseItemOfficialRatingMigration, AddBaseItemPremiereDateMigration,
    AddUserPolicyProvidersMigration, CreateUsersMigration, OptimizeYearQueriesMigration,
};
use sea_orm::{ConnectionTrait, Statement, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

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
    AddUserPolicyProvidersMigration
        .up(&schema)
        .await
        .expect("reapplying the provider-column DDL must succeed");
    AddUserPolicyProvidersMigration
        .up(&schema)
        .await
        .expect("the provider-column DDL must remain idempotent");
    OptimizeYearQueriesMigration
        .up(&schema)
        .await
        .expect("reapplying the year-query DDL must succeed");
    OptimizeYearQueriesMigration
        .up(&schema)
        .await
        .expect("the year-query DDL must remain idempotent");
    AddBaseItemOfficialRatingMigration
        .up(&schema)
        .await
        .expect("reapplying the official-rating DDL must succeed");
    AddBaseItemOfficialRatingMigration
        .up(&schema)
        .await
        .expect("the official-rating DDL must remain idempotent");
    AddBaseItemPremiereDateMigration
        .up(&schema)
        .await
        .expect("reapplying the premiere-date DDL must succeed");
    AddBaseItemPremiereDateMigration
        .up(&schema)
        .await
        .expect("the premiere-date DDL must remain idempotent");

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

    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'base_items'"
                .to_owned(),
        ))
        .await
        .expect("PostgreSQL base item catalog query must succeed");
    let base_item_index_names: Vec<String> = rows
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name must be text"))
        .collect();
    assert!(
        base_item_index_names
            .iter()
            .any(|name| name == "base_items_production_year_idx")
    );
    assert!(
        base_item_index_names
            .iter()
            .any(|name| name == "base_items_official_rating_idx")
    );
    assert!(
        base_item_index_names
            .iter()
            .any(|name| name == "base_items_episode_premiere_date_idx")
    );

    let columns = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT column_name, data_type, character_maximum_length, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' AND table_name = 'users' \
               AND column_name IN (\
                   'authentication_provider_id', 'password_reset_provider_id'\
               ) \
             ORDER BY column_name"
                .to_owned(),
        ))
        .await
        .expect("provider column catalog query must succeed");
    assert_eq!(columns.len(), 2);
    for row in &columns {
        let name = String::try_get(row, "", "column_name").expect("column name must be text");
        let data_type = String::try_get(row, "", "data_type").expect("data type must be text");
        let max_length = i32::try_get(row, "", "character_maximum_length")
            .expect("provider identifier length must be numeric");
        let nullable = String::try_get(row, "", "is_nullable").expect("nullability must be text");
        let default = String::try_get(row, "", "column_default").expect("default must be text");

        assert_eq!(data_type, "character varying");
        assert_eq!(max_length, 255);
        assert_eq!(nullable, "NO");
        let expected = match name.as_str() {
            "authentication_provider_id" => {
                "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
            }
            "password_reset_provider_id" => {
                "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider"
            }
            _ => panic!("unexpected provider column: {name}"),
        };
        assert!(default.contains(expected), "unexpected default: {default}");
    }

    let constraints = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT conname FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.users'::regclass \
               AND conname IN (\
                   'users_authentication_provider_not_blank',\
                   'users_password_reset_provider_not_blank'\
               )"
            .to_owned(),
        ))
        .await
        .expect("provider constraint catalog query must succeed");
    assert_eq!(constraints.len(), 2);
}

#[tokio::test]
async fn provider_migration_backfills_and_synchronizes_existing_policy_json() {
    let administrator = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("jellyfin_provider_{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name, "jellyfin_provider_");
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");
    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_provider_migration(&task_database_name).await;
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

async fn exercise_provider_migration(database_name: &str) {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    let schema = SchemaManager::new(&database);
    CreateUsersMigration
        .up(&schema)
        .await
        .expect("legacy users schema creation must succeed");

    let valid_id = Uuid::new_v4();
    let invalid_id = Uuid::new_v4();
    database
        .execute_unprepared(&format!(
            r"
            INSERT INTO jellyfin.users
                (id, username, normalized_username, policy)
            VALUES
                ('{valid_id}', 'provider-valid', 'PROVIDER-VALID',
                 jsonb_build_object(
                    'AuthenticationProviderId', 'Custom.Authentication',
                    'PasswordResetProviderId', 'Custom.PasswordReset'
                 )),
                ('{invalid_id}', 'provider-invalid', 'PROVIDER-INVALID',
                 jsonb_build_object(
                    'AuthenticationProviderId', '   ',
                    'PasswordResetProviderId', repeat('x', 256)
                 ));
            "
        ))
        .await
        .expect("legacy policy fixtures must insert");

    AddUserPolicyProvidersMigration
        .up(&schema)
        .await
        .expect("provider migration must succeed");
    AddUserPolicyProvidersMigration
        .up(&schema)
        .await
        .expect("provider migration reapplication must succeed");

    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT id::text, authentication_provider_id, password_reset_provider_id, policy \
             FROM jellyfin.users ORDER BY normalized_username"
                .to_owned(),
        ))
        .await
        .expect("migrated provider rows must be queryable");
    assert_eq!(rows.len(), 2);

    let invalid = &rows[0];
    let invalid_auth = String::try_get(invalid, "", "authentication_provider_id").unwrap();
    let invalid_reset = String::try_get(invalid, "", "password_reset_provider_id").unwrap();
    let invalid_policy = serde_json::Value::try_get(invalid, "", "policy").unwrap();
    assert_eq!(
        invalid_auth,
        "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
    );
    assert_eq!(
        invalid_reset,
        "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider"
    );
    assert_eq!(invalid_policy["AuthenticationProviderId"], invalid_auth);
    assert_eq!(invalid_policy["PasswordResetProviderId"], invalid_reset);

    let valid = &rows[1];
    let valid_auth = String::try_get(valid, "", "authentication_provider_id").unwrap();
    let valid_reset = String::try_get(valid, "", "password_reset_provider_id").unwrap();
    let valid_policy = serde_json::Value::try_get(valid, "", "policy").unwrap();
    assert_eq!(valid_auth, "Custom.Authentication");
    assert_eq!(valid_reset, "Custom.PasswordReset");
    assert_eq!(valid_policy["AuthenticationProviderId"], valid_auth);
    assert_eq!(valid_policy["PasswordResetProviderId"], valid_reset);

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

fn assert_temporary_database_name(name: &str, prefix: &str) {
    let suffix = name
        .strip_prefix(prefix)
        .expect("temporary database name must have its fixed prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
