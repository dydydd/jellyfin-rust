use jellyfin_data::{DatabaseConfig, NamedConfigurationRepository, NamedConfigurationStoreError};
use sea_orm::ConnectionTrait;
use serde_json::json;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_named_configurations_";

#[tokio::test]
async fn postgres_named_configurations_are_canonical_versioned_and_object_typed() {
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
        exercise_named_configurations(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_named_configurations(database_name: &str) {
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

    let repository = NamedConfigurationRepository::new(database.clone());
    assert!(matches!(
        repository.load(" \t ").await,
        Err(NamedConfigurationStoreError::BlankKey)
    ));
    assert!(matches!(
        repository.load("branding").await,
        Err(NamedConfigurationStoreError::NotFound(key)) if key == "branding"
    ));

    let created = repository
        .save(
            " Branding ",
            json!({
                "LoginDisclaimer": "Hello",
                "SplashscreenEnabled": true
            }),
        )
        .await
        .expect("named configuration insert");
    assert_eq!(created.key, "branding");
    assert_eq!(created.row_version, 1);

    let loaded = repository
        .load("BRANDING")
        .await
        .expect("case-insensitive load");
    assert_eq!(loaded, created);

    let replaced = repository
        .save(
            "branding",
            json!({
                "LoginDisclaimer": "Updated",
                "CustomCss": "body { color: #00a4dc; }"
            }),
        )
        .await
        .expect("named configuration upsert");
    assert_eq!(replaced.key, "branding");
    assert_eq!(replaced.configuration["LoginDisclaimer"], "Updated");
    assert_eq!(replaced.created_at, created.created_at);
    assert!(replaced.row_version > created.row_version);

    assert!(
        repository
            .save("branding", json!(["not", "object"]))
            .await
            .is_err(),
        "PostgreSQL constraint must reject non-object named configurations"
    );

    database.close().await.expect("database pool cleanup");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
