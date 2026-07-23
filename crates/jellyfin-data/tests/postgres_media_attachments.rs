use std::collections::BTreeMap;

use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, MediaAttachmentQuery, MediaAttachmentRepository,
    MediaAttachmentStoreError, NewBaseItem, PersistedMediaAttachment,
    entities::{base_item, media_attachment},
};
use jellyfin_migration::CreateMediaAttachmentsMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, ModelTrait, QueryResult,
    Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_media_attachments_";

#[tokio::test]
async fn postgres_media_attachments_are_atomic_complete_and_queryable() {
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
        exercise_media_attachments(&task_database_name).await;
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

async fn exercise_media_attachments(database_name: &str) {
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
    CreateMediaAttachmentsMigration
        .up(&schema)
        .await
        .expect("reapplying media-attachment DDL must succeed");
    CreateMediaAttachmentsMigration
        .up(&schema)
        .await
        .expect("media-attachment DDL must remain idempotent");

    let items = BaseItemRepository::new(database.clone());
    let attachments = MediaAttachmentRepository::new(database.clone());
    assert_full_and_null_roundtrip(&database, &items, &attachments).await;
    assert_replace_stale_and_clear(&items, &attachments).await;
    assert_filters_and_sorting(&items, &attachments).await;
    assert_batch_query_for_items(&items, &attachments).await;
    assert_duplicate_and_missing_preserve_data(&items, &attachments).await;
    assert_cascade(&database, &items, &attachments).await;
    assert_concurrent_replacements_are_complete(&items, &attachments).await;
    assert_postgres_catalog(&database).await;
    assert_item_query_plans(&database, &items, &attachments).await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_full_and_null_roundtrip(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "roundtrip").await;
    let values = vec![minimal_attachment(-1), full_attachment(0)];

    assert_eq!(attachments.replace(item.id, &values).await.unwrap(), values);
    let restarted = MediaAttachmentRepository::new(database.clone());
    assert_eq!(
        restarted
            .query(MediaAttachmentQuery::for_item(item.id))
            .await
            .unwrap(),
        values
    );
    assert_eq!(
        item.find_related(media_attachment::Entity)
            .all(database)
            .await
            .expect("SeaORM media-attachment relation")
            .len(),
        2
    );
    assert_eq!(
        values[0].attachment_index, -1,
        "sentinel index must roundtrip"
    );
    assert!(values[0].codec.is_none());
    assert!(values[0].delivery_url.is_none());
}

async fn assert_replace_stale_and_clear(
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "replace").await;
    let original = vec![
        minimal_attachment(0),
        full_attachment(1),
        attachment(2, "poster.jpg"),
    ];
    attachments.replace(item.id, &original).await.unwrap();

    let mut updated = full_attachment(1);
    updated.comment = Some("Updated poster".to_owned());
    let replacement = vec![updated, attachment(3, "logo.png")];
    assert_eq!(
        attachments.replace(item.id, &replacement).await.unwrap(),
        replacement
    );
    assert_eq!(
        attachments
            .query(MediaAttachmentQuery::for_item(item.id))
            .await
            .unwrap(),
        replacement
    );

    assert!(attachments.replace(item.id, &[]).await.unwrap().is_empty());
    assert!(
        attachments
            .query(MediaAttachmentQuery::for_item(item.id))
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_filters_and_sorting(
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "filters").await;
    let values = vec![
        attachment(7, "logo.png"),
        attachment(-1, "poster.jpg"),
        attachment(3, "backdrop.webp"),
    ];
    let stored = attachments.replace(item.id, &values).await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|attachment| attachment.attachment_index)
            .collect::<Vec<_>>(),
        [-1, 3, 7]
    );
    assert_eq!(
        attachments
            .query(MediaAttachmentQuery {
                item_id: item.id,
                attachment_index: Some(3),
            })
            .await
            .unwrap(),
        [attachment(3, "backdrop.webp")]
    );
    assert!(
        attachments
            .query(MediaAttachmentQuery {
                item_id: item.id,
                attachment_index: Some(42),
            })
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_batch_query_for_items(
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let first = create_item(items, "batch-first").await;
    let second = create_item(items, "batch-second").await;
    let missing = Uuid::new_v4();
    let first_values = vec![attachment(1, "poster.jpg"), attachment(3, "logo.png")];
    let second_values = vec![attachment(-1, "cover.webp")];
    attachments.replace(first.id, &first_values).await.unwrap();
    attachments
        .replace(second.id, &second_values)
        .await
        .unwrap();

    let batch = attachments
        .query_for_items(&[second.id, first.id, missing])
        .await
        .unwrap();
    assert_eq!(batch.get(&first.id).cloned().unwrap(), first_values);
    assert_eq!(batch.get(&second.id).cloned().unwrap(), second_values);
    assert!(!batch.contains_key(&missing));
    assert!(attachments.query_for_items(&[]).await.unwrap().is_empty());
}

async fn assert_duplicate_and_missing_preserve_data(
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "errors").await;
    let valid = attachment(0, "poster.jpg");
    attachments
        .replace(item.id, std::slice::from_ref(&valid))
        .await
        .unwrap();

    let duplicate = [attachment(1, "poster.jpg"), attachment(1, "logo.png")];
    assert!(matches!(
        attachments.replace(item.id, &duplicate).await,
        Err(MediaAttachmentStoreError::DuplicateAttachmentIndex {
            attachment_index: 1
        })
    ));
    assert!(matches!(
        attachments
            .replace(Uuid::new_v4(), std::slice::from_ref(&valid))
            .await,
        Err(MediaAttachmentStoreError::BaseItemNotFound { .. })
    ));
    assert_eq!(
        attachments
            .query(MediaAttachmentQuery::for_item(item.id))
            .await
            .unwrap(),
        [valid]
    );
}

async fn assert_cascade(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "cascade").await;
    attachments
        .replace(item.id, &[attachment(0, "poster.jpg")])
        .await
        .unwrap();
    items.delete(item.id).await.expect("base-item deletion");
    assert!(
        media_attachment::Entity::find()
            .all(database)
            .await
            .expect("media-attachment scan")
            .into_iter()
            .all(|row| row.item_id != item.id)
    );
}

async fn assert_concurrent_replacements_are_complete(
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "concurrent").await;
    let first_values = vec![attachment(0, "poster.jpg"), attachment(1, "logo.png")];
    let second_values = vec![attachment(2, "backdrop.webp")];
    let first = attachments.clone();
    let second = attachments.clone();
    let (first_result, second_result) = tokio::join!(
        first.replace(item.id, &first_values),
        second.replace(item.id, &second_values)
    );
    first_result.expect("first replacement");
    second_result.expect("second replacement");

    let current = attachments
        .query(MediaAttachmentQuery::for_item(item.id))
        .await
        .unwrap();
    assert!(
        current == first_values || current == second_values,
        "concurrent replacement left a partial set: {current:?}"
    );
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT conname, pg_get_constraintdef(oid) AS definition \
             FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.media_attachments'::regclass \
             ORDER BY conname"
                .to_owned(),
        ))
        .await
        .expect("media-attachment constraint catalog")
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
        ["media_attachments_item_id_fkey", "media_attachments_pkey"]
    );
    assert_eq!(
        constraints["media_attachments_pkey"],
        "PRIMARY KEY (item_id, attachment_index)"
    );
    assert!(constraints["media_attachments_item_id_fkey"].contains("ON DELETE CASCADE"));

    let columns = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name, is_nullable \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' AND table_name = 'media_attachments' \
             ORDER BY ordinal_position"
                .to_owned(),
        ))
        .await
        .expect("media-attachment column catalog");
    assert_eq!(columns.len(), 8);
    let names = columns
        .iter()
        .map(|row| String::try_get(row, "", "column_name").unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "item_id",
            "attachment_index",
            "codec",
            "codec_tag",
            "comment",
            "file_name",
            "mime_type",
            "delivery_url",
        ]
    );
    for index in [0, 1] {
        assert_eq!(
            String::try_get(&columns[index], "", "is_nullable").unwrap(),
            "NO"
        );
    }

    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'media_attachments'"
                .to_owned(),
        ))
        .await
        .expect("media-attachment index catalog");
    assert_eq!(
        indexes
            .iter()
            .map(|row| String::try_get(row, "", "indexname").unwrap())
            .collect::<Vec<_>>(),
        ["media_attachments_pkey"]
    );
}

async fn assert_item_query_plans(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    attachments: &MediaAttachmentRepository,
) {
    let item = create_item(items, "explain").await;
    let values = (0_i32..128)
        .map(|index| attachment(index, &format!("poster-{index}.jpg")))
        .collect::<Vec<_>>();
    attachments.replace(item.id, &values).await.unwrap();
    database
        .execute_unprepared("ANALYZE jellyfin.media_attachments")
        .await
        .expect("analyze media attachments");
    let transaction = database.begin().await.expect("EXPLAIN transaction");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");
    for (sql, values) in [
        (
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.media_attachments \
             WHERE item_id = $1 AND attachment_index = $2 ORDER BY attachment_index",
            vec![item.id.into(), 42_i32.into()],
        ),
        (
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.media_attachments \
             WHERE item_id = $1 ORDER BY attachment_index",
            vec![item.id.into()],
        ),
    ] {
        let plan = transaction
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .expect("media-attachment EXPLAIN")
            .iter()
            .map(explain_line)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            plan.contains("media_attachments_pkey"),
            "expected primary key plan without another write-heavy index:\n{plan}"
        );
    }
    transaction.rollback().await.expect("EXPLAIN rollback");
}

fn explain_line(row: &QueryResult) -> String {
    String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line must be text")
}

async fn create_item(items: &BaseItemRepository, label: &str) -> base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Video");
    item.name = Some(label.to_owned());
    item.sort_name = Some(label.to_owned());
    items.create(item).await.expect("base-item creation")
}

fn minimal_attachment(attachment_index: i32) -> PersistedMediaAttachment {
    PersistedMediaAttachment {
        attachment_index,
        codec: None,
        codec_tag: None,
        comment: None,
        file_name: None,
        mime_type: None,
        delivery_url: None,
    }
}

fn attachment(attachment_index: i32, file_name: &str) -> PersistedMediaAttachment {
    PersistedMediaAttachment {
        attachment_index,
        file_name: Some(file_name.to_owned()),
        ..minimal_attachment(attachment_index)
    }
}

fn full_attachment(attachment_index: i32) -> PersistedMediaAttachment {
    PersistedMediaAttachment {
        attachment_index,
        codec: Some("mjpeg".to_owned()),
        codec_tag: Some("MJPG".to_owned()),
        comment: Some("poster".to_owned()),
        file_name: Some("poster.jpg".to_owned()),
        mime_type: Some("image/jpeg".to_owned()),
        delivery_url: Some("/Videos/item/Attachments/0".to_owned()),
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
