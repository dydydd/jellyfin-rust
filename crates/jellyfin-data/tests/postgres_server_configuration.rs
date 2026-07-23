use jellyfin_data::{
    DatabaseConfig, ServerConfigurationRepository, ServerConfigurationStoreError,
    StartupConfigurationUpdate,
};
use jellyfin_migration::{
    AddClientLogUploadConfigurationMigration, AddPlaystateResumeConfigurationMigration,
    CreateServerConfigurationMigration,
};
use sea_orm::{ConnectionTrait, EntityTrait, PaginatorTrait, Statement, TryGetable};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
    assert_configuration_migrations_are_idempotent(&schema).await;
    assert_singleton_schema(&database).await;

    let first = ServerConfigurationRepository::new(database.clone());
    let second = ServerConfigurationRepository::new(database.clone());
    let seeded = first.load().await.expect("seeded singleton");
    assert_eq!(seeded.id, 1);
    assert_eq!(seeded.server_name, "Jellyfin");
    assert!(!seeded.is_startup_wizard_completed);
    assert_eq!(seeded.content_types, json!([]));
    assert_eq!(seeded.min_resume_pct, 5);
    assert_eq!(seeded.max_resume_pct, 90);
    assert_eq!(seeded.min_resume_duration_seconds, 300);
    assert_eq!(seeded.min_audiobook_resume, 5);
    assert_eq!(seeded.max_audiobook_resume, 5);
    assert!(seeded.allow_client_log_upload);
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

    assert_content_type_updates(&database, &first, &second).await;
    assert_client_log_upload_updates(&first, &second).await;

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
    assert!(matches!(
        first
            .update_content_type_override("/media/movies", Some("movies"))
            .await,
        Err(ServerConfigurationStoreError::MissingSingleton)
    ));
    assert!(matches!(
        first.update_client_log_upload(true).await,
        Err(ServerConfigurationStoreError::MissingSingleton)
    ));

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_configuration_migrations_are_idempotent(schema: &SchemaManager<'_>) {
    CreateServerConfigurationMigration
        .up(schema)
        .await
        .expect("reapplying server-configuration DDL must succeed");
    CreateServerConfigurationMigration
        .up(schema)
        .await
        .expect("server-configuration DDL must remain idempotent");
    AddPlaystateResumeConfigurationMigration
        .up(schema)
        .await
        .expect("reapplying playstate configuration DDL must succeed");
    AddPlaystateResumeConfigurationMigration
        .up(schema)
        .await
        .expect("playstate configuration DDL must remain idempotent");
    AddClientLogUploadConfigurationMigration
        .up(schema)
        .await
        .expect("reapplying client-log configuration DDL must succeed");
    AddClientLogUploadConfigurationMigration
        .up(schema)
        .await
        .expect("client-log configuration DDL must remain idempotent");
}

async fn assert_content_type_updates(
    database: &sea_orm::DatabaseConnection,
    first: &ServerConfigurationRepository,
    second: &ServerConfigurationRepository,
) {
    first
        .update_content_type_override("/Media/Movies", Some("movies"))
        .await
        .expect("initial content-type override");
    let replaced = second
        .update_content_type_override("/media/movies", Some("tvshows"))
        .await
        .expect("case-insensitive replacement");
    assert_eq!(
        content_types(&replaced.content_types),
        BTreeMap::from([("/media/movies".to_owned(), "tvshows".to_owned())])
    );

    let removed = first
        .update_content_type_override("/MEDIA/MOVIES", Some("  \t"))
        .await
        .expect("whitespace removes override");
    assert!(content_types(&removed.content_types).is_empty());

    let (movies, music) = tokio::join!(
        first.update_content_type_override("/library/movies", Some("movies")),
        second.update_content_type_override("/library/music", Some("music")),
    );
    movies.expect("concurrent movies override");
    music.expect("concurrent music override");
    let restarted = ServerConfigurationRepository::new(database.clone());
    let persisted = restarted.load().await.expect("persisted content types");
    assert_eq!(
        content_types(&persisted.content_types),
        BTreeMap::from([
            ("/library/movies".to_owned(), "movies".to_owned()),
            ("/library/music".to_owned(), "music".to_owned()),
        ])
    );
}

async fn assert_client_log_upload_updates(
    first: &ServerConfigurationRepository,
    second: &ServerConfigurationRepository,
) {
    let disabled = first
        .update_client_log_upload(false)
        .await
        .expect("client-log upload disable");
    assert!(!disabled.allow_client_log_upload);

    let enabled = second
        .update_client_log_upload(true)
        .await
        .expect("client-log upload enable");
    assert!(enabled.allow_client_log_upload);
    assert!(enabled.row_version > disabled.row_version);
}

fn content_types(value: &Value) -> BTreeMap<String, String> {
    value
        .as_array()
        .expect("content types must be a JSON array")
        .iter()
        .map(|entry| {
            (
                entry["Name"]
                    .as_str()
                    .expect("content-type name")
                    .to_owned(),
                entry["Value"]
                    .as_str()
                    .expect("content-type value")
                    .to_owned(),
            )
        })
        .collect()
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

    let content_types = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' \
               AND table_name = 'server_configuration' \
               AND column_name = 'content_types'"
                .to_owned(),
        ))
        .await
        .expect("content-types column catalog query")
        .expect("content-types column");
    assert_eq!(
        String::try_get(&content_types, "", "data_type").unwrap(),
        "jsonb"
    );
    assert_eq!(
        String::try_get(&content_types, "", "is_nullable").unwrap(),
        "NO"
    );
    assert_eq!(
        String::try_get(&content_types, "", "column_default").unwrap(),
        "'[]'::jsonb"
    );

    assert_playstate_resume_schema(database).await;
    assert_client_log_upload_schema(database).await;

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

    let row = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT count(*)::bigint AS count FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.server_configuration'::regclass \
               AND conname = 'server_configuration_content_types_array'"
                .to_owned(),
        ))
        .await
        .expect("content-types constraint catalog query")
        .expect("content-types constraint count row");
    assert_eq!(i64::try_get(&row, "", "count").unwrap(), 1);
}

async fn assert_client_log_upload_schema(database: &sea_orm::DatabaseConnection) {
    let column = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' \
               AND table_name = 'server_configuration' \
               AND column_name = 'allow_client_log_upload'"
                .to_owned(),
        ))
        .await
        .expect("client-log upload column catalog query")
        .expect("client-log upload column");
    assert_eq!(
        String::try_get(&column, "", "data_type").unwrap(),
        "boolean"
    );
    assert_eq!(String::try_get(&column, "", "is_nullable").unwrap(), "NO");
    assert_eq!(
        String::try_get(&column, "", "column_default").unwrap(),
        "true"
    );
}

async fn assert_playstate_resume_schema(database: &sea_orm::DatabaseConnection) {
    let resume_columns = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' \
               AND table_name = 'server_configuration' \
               AND column_name IN (\
                   'min_resume_pct', 'max_resume_pct', \
                   'min_resume_duration_seconds', \
                   'min_audiobook_resume', 'max_audiobook_resume'\
               )"
            .to_owned(),
        ))
        .await
        .expect("playstate resume column catalog query")
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "column_name").unwrap(),
                (
                    String::try_get(&row, "", "data_type").unwrap(),
                    String::try_get(&row, "", "is_nullable").unwrap(),
                    String::try_get(&row, "", "column_default").unwrap(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (name, default_value) in [
        ("min_resume_pct", "5"),
        ("max_resume_pct", "90"),
        ("min_resume_duration_seconds", "300"),
        ("min_audiobook_resume", "5"),
        ("max_audiobook_resume", "5"),
    ] {
        let (data_type, is_nullable, column_default) = resume_columns
            .get(name)
            .unwrap_or_else(|| panic!("missing resume configuration column {name}"));
        assert_eq!(data_type, "integer");
        assert_eq!(is_nullable, "NO");
        assert_eq!(column_default, default_value);
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
