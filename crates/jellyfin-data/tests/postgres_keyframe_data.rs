use std::collections::BTreeMap;

use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, KeyframeDataRecord, KeyframeDataRepository,
    KeyframeDataStoreError, NewBaseItem, NewKeyframeData,
    entities::{base_item, keyframe_data},
};
use jellyfin_migration::CreateKeyframeDataMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, ModelTrait, Statement, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::json;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_keyframes_";

#[tokio::test]
async fn postgres_keyframe_data_is_atomic_typed_and_backup_safe() {
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
        exercise_keyframe_data(&task_database_name).await;
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

async fn exercise_keyframe_data(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    CreateKeyframeDataMigration
        .up(&schema)
        .await
        .expect("reapplying keyframe DDL must succeed");
    CreateKeyframeDataMigration
        .up(&schema)
        .await
        .expect("keyframe DDL must remain idempotent");

    let items = BaseItemRepository::new(database.clone());
    let keyframes = KeyframeDataRepository::new(database.clone());
    assert_save_reload_replace_and_delete(&database, &items, &keyframes).await;
    assert_official_duration_semantics(&items, &keyframes).await;
    assert_missing_item_is_typed(&keyframes).await;
    assert_cascade(&database, &items, &keyframes).await;
    assert_concurrent_saves_are_complete(&items, &keyframes).await;
    assert_backup_export_skips_corrupt_rows(&database, &items, &keyframes).await;
    assert_database_constraints(&database, &items, &keyframes).await;
    assert_postgres_catalog(&database).await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_save_reload_replace_and_delete(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    keyframes: &KeyframeDataRepository,
) {
    let item = create_item(items, "replace").await;
    let first = data(60_000, &[0, 1_000, 2_000]);
    assert_eq!(
        keyframes.save(item.id, first.clone()).await.unwrap(),
        record(item.id, &first)
    );

    let restarted = KeyframeDataRepository::new(database.clone());
    assert_eq!(
        restarted.get(item.id).await.unwrap(),
        Some(record(item.id, &first))
    );
    assert_eq!(
        item.find_related(keyframe_data::Entity)
            .one(database)
            .await
            .expect("SeaORM keyframe relation")
            .expect("related keyframe row")
            .item_id,
        item.id
    );

    let replacement = data(90_000, &[0, 30_000, 60_000]);
    assert_eq!(
        restarted.save(item.id, replacement.clone()).await.unwrap(),
        record(item.id, &replacement)
    );
    assert_eq!(
        restarted.get(item.id).await.unwrap(),
        Some(record(item.id, &replacement))
    );

    assert!(restarted.delete(item.id).await.unwrap());
    assert!(!restarted.delete(item.id).await.unwrap());
    assert_eq!(restarted.get(item.id).await.unwrap(), None);
}

async fn assert_official_duration_semantics(
    items: &BaseItemRepository,
    keyframes: &KeyframeDataRepository,
) {
    let item = create_item(items, "official-semantics").await;
    for value in [
        data(9_900, &[0, 5_000, 10_000]),
        data(0, &[10_000]),
        data(i64::MIN, &[i64::MAX, -1, i64::MIN, i64::MAX]),
    ] {
        let stored = keyframes.save(item.id, value.clone()).await.unwrap();
        assert_eq!(stored, record(item.id, &value));
    }
}

async fn assert_missing_item_is_typed(keyframes: &KeyframeDataRepository) {
    let item_id = Uuid::new_v4();
    assert!(matches!(
        keyframes.save(item_id, data(1, &[0])).await,
        Err(KeyframeDataStoreError::BaseItemNotFound { item_id: missing })
            if missing == item_id
    ));
}

async fn assert_cascade(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    keyframes: &KeyframeDataRepository,
) {
    let item = create_item(items, "cascade").await;
    keyframes
        .save(item.id, data(10_000, &[0, 5_000]))
        .await
        .unwrap();
    base_item::Entity::delete_by_id(item.id)
        .exec(database)
        .await
        .expect("base-item deletion");
    assert_eq!(keyframes.get(item.id).await.unwrap(), None);
}

async fn assert_concurrent_saves_are_complete(
    items: &BaseItemRepository,
    keyframes: &KeyframeDataRepository,
) {
    let item = create_item(items, "concurrent").await;
    let first = data(100, &[0, 25, 50, 75]);
    let second = data(200, &[0, 80, 160, 240]);
    let concurrent = keyframes.clone();
    let (first_result, second_result) = tokio::join!(
        keyframes.save(item.id, first.clone()),
        concurrent.save(item.id, second.clone()),
    );
    assert_eq!(first_result.unwrap(), record(item.id, &first));
    assert_eq!(second_result.unwrap(), record(item.id, &second));
    let final_row = keyframes.get(item.id).await.unwrap().unwrap();
    assert!(
        final_row == record(item.id, &first) || final_row == record(item.id, &second),
        "concurrent upsert must leave one complete input: {final_row:?}"
    );
}

async fn assert_backup_export_skips_corrupt_rows(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    keyframes: &KeyframeDataRepository,
) {
    let valid_item = create_item(items, "backup-valid").await;
    let corrupt_item = create_item(items, "backup-corrupt").await;
    let valid = data(60_000, &[0, 1_000, 2_000]);
    keyframes.save(valid_item.id, valid.clone()).await.unwrap();
    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.keyframe_data \
             (item_id, total_duration, keyframe_ticks) \
             VALUES ($1, 5000, $2)",
            [corrupt_item.id.into(), json!([0, "bad", 2_000]).into()],
        ))
        .await
        .expect("semantically corrupt JSONB array must satisfy structural constraint");

    assert!(matches!(
        keyframes.get(corrupt_item.id).await,
        Err(KeyframeDataStoreError::CorruptTicks { item_id, .. })
            if item_id == corrupt_item.id
    ));
    let export = keyframes.export_valid().await.expect("backup export scan");
    assert!(export.records.contains(&record(valid_item.id, &valid)));
    assert!(
        !export
            .records
            .iter()
            .any(|row| row.item_id == corrupt_item.id)
    );
    assert_eq!(export.skipped_item_ids, [corrupt_item.id]);

    assert!(keyframes.delete(corrupt_item.id).await.unwrap());
    let clean_export = keyframes.export_valid().await.unwrap();
    assert!(clean_export.skipped_item_ids.is_empty());
}

async fn assert_database_constraints(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    keyframes: &KeyframeDataRepository,
) {
    let item = create_item(items, "constraints").await;
    for (label, value) in [
        ("object", json!({ "tick": 1 })),
        ("scalar", json!(1)),
        ("string", json!("ticks")),
        ("JSON null", serde_json::Value::Null),
    ] {
        let result = database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO jellyfin.keyframe_data \
                 (item_id, total_duration, keyframe_ticks) \
                 VALUES ($1, 1, $2)",
                [item.id.into(), value.into()],
            ))
            .await;
        assert!(result.is_err(), "database must reject {label} ticks");
    }

    let null_ticks = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.keyframe_data \
             (item_id, total_duration, keyframe_ticks) VALUES ($1, 1, NULL)",
            [item.id.into()],
        ))
        .await;
    assert!(null_ticks.is_err(), "database must reject SQL NULL ticks");

    let null_duration = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.keyframe_data \
             (item_id, total_duration, keyframe_ticks) VALUES ($1, NULL, '[]')",
            [item.id.into()],
        ))
        .await;
    assert!(null_duration.is_err(), "database must reject NULL duration");

    let orphan = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.keyframe_data \
             (item_id, total_duration, keyframe_ticks) VALUES ($1, 1, '[]')",
            [Uuid::new_v4().into()],
        ))
        .await;
    assert!(orphan.is_err(), "database FK must reject orphan data");

    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.keyframe_data (item_id, total_duration) VALUES ($1, 1)",
            [item.id.into()],
        ))
        .await
        .expect("JSONB default must be an empty array");
    assert_eq!(
        keyframes.get(item.id).await.unwrap(),
        Some(KeyframeDataRecord {
            item_id: item.id,
            total_duration: 1,
            keyframe_ticks: Vec::new(),
        })
    );
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT conname, pg_get_constraintdef(oid) AS definition \
             FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.keyframe_data'::regclass \
             ORDER BY conname"
                .to_owned(),
        ))
        .await
        .expect("keyframe constraint catalog")
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "conname").unwrap(),
                String::try_get(&row, "", "definition").unwrap(),
            )
        })
        .collect::<BTreeMap<String, String>>();
    assert_eq!(
        constraints.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "keyframe_data_item_id_fkey",
            "keyframe_data_pkey",
            "keyframe_data_ticks_array"
        ]
    );
    assert_eq!(constraints["keyframe_data_pkey"], "PRIMARY KEY (item_id)");
    assert!(constraints["keyframe_data_item_id_fkey"].contains("ON DELETE CASCADE"));
    assert!(constraints["keyframe_data_ticks_array"].contains("jsonb_typeof"));

    let columns = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' AND table_name = 'keyframe_data' \
             ORDER BY ordinal_position"
                .to_owned(),
        ))
        .await
        .expect("keyframe column catalog");
    assert_eq!(columns.len(), 3);
    let types = columns
        .iter()
        .map(|row| {
            (
                String::try_get(row, "", "column_name").unwrap(),
                String::try_get(row, "", "data_type").unwrap(),
                String::try_get(row, "", "is_nullable").unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        types,
        [
            ("item_id".to_owned(), "uuid".to_owned(), "NO".to_owned()),
            (
                "total_duration".to_owned(),
                "bigint".to_owned(),
                "NO".to_owned()
            ),
            (
                "keyframe_ticks".to_owned(),
                "jsonb".to_owned(),
                "NO".to_owned()
            ),
        ]
    );
    let ticks_default =
        String::try_get(&columns[2], "", "column_default").expect("ticks default must be text");
    assert!(ticks_default.contains("'[]'::jsonb"));

    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'keyframe_data'"
                .to_owned(),
        ))
        .await
        .expect("keyframe index catalog");
    assert_eq!(indexes.len(), 1);
    assert_eq!(
        String::try_get(&indexes[0], "", "indexname").unwrap(),
        "keyframe_data_pkey"
    );
}

async fn create_item(items: &BaseItemRepository, label: &str) -> base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Movie");
    item.name = Some(label.to_owned());
    item.sort_name = Some(label.to_owned());
    items.create(item).await.expect("base-item creation")
}

fn data(total_duration: i64, keyframe_ticks: &[i64]) -> NewKeyframeData {
    NewKeyframeData {
        total_duration,
        keyframe_ticks: keyframe_ticks.to_vec(),
    }
}

fn record(item_id: Uuid, data: &NewKeyframeData) -> KeyframeDataRecord {
    KeyframeDataRecord {
        item_id,
        total_duration: data.total_duration,
        keyframe_ticks: data.keyframe_ticks.clone(),
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
