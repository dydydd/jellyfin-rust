use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemImage, BaseItemImageRepository, BaseItemImageStoreError, BaseItemImageType,
    BaseItemRepository, DatabaseConfig, NewBaseItem, NewBaseItemImage,
    entities::{base_item, base_item_image},
};
use jellyfin_migration::CreateBaseItemImagesMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, ModelTrait, QueryResult,
    Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_item_images_";

#[tokio::test]
async fn postgres_base_item_images_are_atomic_typed_and_indexed() {
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
        exercise_base_item_images(&task_database_name).await;
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

async fn exercise_base_item_images(database_name: &str) {
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
    CreateBaseItemImagesMigration
        .up(&schema)
        .await
        .expect("reapplying base-item image DDL must succeed");
    CreateBaseItemImagesMigration
        .up(&schema)
        .await
        .expect("base-item image DDL must remain idempotent");

    let items = BaseItemRepository::new(database.clone());
    let images = BaseItemImageRepository::new(database.clone());
    assert_replace_reload_and_clear(&database, &items, &images).await;
    assert_all_image_types_and_list_many(&items, &images).await;
    assert_validation_is_typed(&items, &images).await;
    assert_database_constraints(&database, &items, &images).await;
    assert_foreign_key_cascade(&database, &items, &images).await;
    assert_concurrent_replacements_are_complete(&items, &images).await;
    assert_postgres_catalog(&database).await;
    assert_primary_lookup_plan(&database, &items, &images).await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_replace_reload_and_clear(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
) {
    let item = create_item(items, "replace").await;
    let first = vec![
        image(BaseItemImageType::Primary, 0, "primary-old.jpg", 1),
        image(BaseItemImageType::Backdrop, 0, "backdrop-0.jpg", 2),
        image(BaseItemImageType::Backdrop, 1, "backdrop-1.jpg", 3),
    ];
    assert_eq!(
        images.replace(item.id, &first).await.unwrap(),
        persisted(item.id, &first)
    );

    let restarted = BaseItemImageRepository::new(database.clone());
    assert_eq!(
        restarted.list(item.id).await.unwrap(),
        persisted(item.id, &first)
    );
    assert_eq!(
        item.find_related(base_item_image::Entity)
            .all(database)
            .await
            .expect("SeaORM base-item image relation")
            .len(),
        first.len()
    );

    let replacement = vec![
        image(BaseItemImageType::Primary, 0, "primary-new.jpg", 4),
        image(BaseItemImageType::Thumb, 0, "thumb.jpg", 5),
    ];
    assert_eq!(
        restarted.replace(item.id, &replacement).await.unwrap(),
        persisted(item.id, &replacement)
    );
    assert_eq!(
        restarted.list(item.id).await.unwrap(),
        persisted(item.id, &replacement)
    );
    assert_eq!(
        restarted.primary(item.id).await.unwrap(),
        Some(persisted(item.id, &replacement).remove(0))
    );

    assert!(restarted.replace(item.id, &[]).await.unwrap().is_empty());
    assert!(restarted.list(item.id).await.unwrap().is_empty());
    assert_eq!(restarted.primary(item.id).await.unwrap(), None);
}

async fn assert_all_image_types_and_list_many(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
) {
    let all_types_item = create_item(items, "all-types").await;
    let all_types = BaseItemImageType::ALL
        .into_iter()
        .enumerate()
        .map(|(offset, image_type)| {
            image(
                image_type,
                0,
                &format!("type-{}.jpg", image_type.as_i16()),
                100 + i64::try_from(offset).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let stored = images.replace(all_types_item.id, &all_types).await.unwrap();
    assert_eq!(stored, persisted(all_types_item.id, &all_types));
    assert_eq!(
        stored
            .iter()
            .map(|image| image.image_type)
            .collect::<Vec<_>>(),
        BaseItemImageType::ALL
    );

    let second_item = create_item(items, "list-many").await;
    let second = vec![image(BaseItemImageType::Primary, 0, "second.jpg", 200)];
    images.replace(second_item.id, &second).await.unwrap();
    let many = images
        .list_many(&[second_item.id, all_types_item.id, second_item.id])
        .await
        .expect("batched image lookup");
    assert_eq!(many.len(), all_types.len() + second.len());
    assert!(many.windows(2).all(|window| {
        (
            window[0].item_id,
            window[0].image_type.as_i16(),
            window[0].image_index,
        ) <= (
            window[1].item_id,
            window[1].image_type.as_i16(),
            window[1].image_index,
        )
    }));
    assert!(images.list_many(&[]).await.unwrap().is_empty());
}

async fn assert_validation_is_typed(items: &BaseItemRepository, images: &BaseItemImageRepository) {
    let item = create_item(items, "validation").await;
    let original = vec![image(BaseItemImageType::Primary, 0, "valid.jpg", 300)];
    images.replace(item.id, &original).await.unwrap();

    let mut blank = image(BaseItemImageType::Primary, 0, " \t ", 301);
    assert!(matches!(
        images.replace(item.id, std::slice::from_ref(&blank)).await,
        Err(BaseItemImageStoreError::EmptyPath)
    ));

    "duplicate.jpg".clone_into(&mut blank.path);
    assert!(matches!(
        images.replace(item.id, &[blank.clone(), blank]).await,
        Err(BaseItemImageStoreError::DuplicateImage {
            image_type: BaseItemImageType::Primary,
            image_index: 0
        })
    ));

    let mut invalid = image(BaseItemImageType::Primary, u32::MAX, "index.jpg", 302);
    assert!(matches!(
        images
            .replace(item.id, std::slice::from_ref(&invalid))
            .await,
        Err(BaseItemImageStoreError::ImageIndexOutOfRange { value: u32::MAX })
    ));
    invalid.image_index = 0;
    invalid.width = Some(0);
    assert!(matches!(
        images
            .replace(item.id, std::slice::from_ref(&invalid))
            .await,
        Err(BaseItemImageStoreError::InvalidDimension {
            field: "width",
            value: 0
        })
    ));
    invalid.width = Some(i32::MAX as u32 + 1);
    assert!(matches!(
        images.replace(item.id, std::slice::from_ref(&invalid)).await,
        Err(BaseItemImageStoreError::InvalidDimension {
            field: "width",
            value
        }) if value == i32::MAX as u32 + 1
    ));
    invalid.width = Some(1);
    invalid.height = Some(0);
    assert!(matches!(
        images
            .replace(item.id, std::slice::from_ref(&invalid))
            .await,
        Err(BaseItemImageStoreError::InvalidDimension {
            field: "height",
            value: 0
        })
    ));

    let missing_id = Uuid::new_v4();
    assert!(matches!(
        images.replace(missing_id, &original).await,
        Err(BaseItemImageStoreError::BaseItemNotFound { item_id }) if item_id == missing_id
    ));
    assert_eq!(
        images.list(item.id).await.unwrap(),
        persisted(item.id, &original)
    );
}

async fn assert_database_constraints(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
) {
    let item = create_item(items, "constraints").await;
    for (label, sql, values) in [
        (
            "image type",
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, $2, 0, 'invalid-type.jpg', clock_timestamp())",
            vec![item.id.into(), 13_i16.into()],
        ),
        (
            "image index",
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, 0, $2, 'invalid-index.jpg', clock_timestamp())",
            vec![item.id.into(), (-1_i32).into()],
        ),
        (
            "blank path",
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, 0, 0, $2, clock_timestamp())",
            vec![item.id.into(), " \t ".into()],
        ),
        (
            "zero width",
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified, width) \
             VALUES ($1, 0, 0, 'zero-width.jpg', clock_timestamp(), $2)",
            vec![item.id.into(), 0_i32.into()],
        ),
        (
            "zero height",
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified, height) \
             VALUES ($1, 0, 0, 'zero-height.jpg', clock_timestamp(), $2)",
            vec![item.id.into(), 0_i32.into()],
        ),
    ] {
        assert!(
            database
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    values
                ))
                .await
                .is_err(),
            "database must reject invalid {label}"
        );
    }

    let missing_path = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, 0, 0, NULL, clock_timestamp())",
            [item.id.into()],
        ))
        .await;
    assert!(missing_path.is_err(), "database must reject a NULL path");

    let missing_date = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, 0, 0, 'no-date.jpg', NULL)",
            [item.id.into()],
        ))
        .await;
    assert!(missing_date.is_err(), "database must reject a NULL date");

    let missing_owner = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, 0, 0, 'orphan.jpg', clock_timestamp())",
            [Uuid::new_v4().into()],
        ))
        .await;
    assert!(
        missing_owner.is_err(),
        "database FK must reject an orphan image"
    );

    let valid = vec![image(BaseItemImageType::Primary, 0, "unique.jpg", 400)];
    images.replace(item.id, &valid).await.unwrap();
    let duplicate = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.base_item_images \
             (item_id, image_type, image_index, path, date_modified) \
             VALUES ($1, 0, 0, 'duplicate.jpg', clock_timestamp())",
            [item.id.into()],
        ))
        .await;
    assert!(
        duplicate.is_err(),
        "composite primary key must reject duplicates"
    );
}

async fn assert_foreign_key_cascade(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
) {
    let item = create_item(items, "cascade").await;
    images
        .replace(
            item.id,
            &[image(BaseItemImageType::Primary, 0, "cascade.jpg", 500)],
        )
        .await
        .unwrap();
    base_item::Entity::delete_by_id(item.id)
        .exec(database)
        .await
        .expect("base-item deletion");
    assert!(images.list(item.id).await.unwrap().is_empty());
}

async fn assert_concurrent_replacements_are_complete(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
) {
    let item = create_item(items, "concurrent").await;
    let first = vec![
        image(BaseItemImageType::Primary, 0, "first-primary.jpg", 600),
        image(BaseItemImageType::Backdrop, 0, "first-backdrop.jpg", 601),
    ];
    let second = vec![
        image(BaseItemImageType::Art, 0, "second-art.jpg", 610),
        image(BaseItemImageType::Logo, 0, "second-logo.jpg", 611),
        image(BaseItemImageType::Thumb, 0, "second-thumb.jpg", 612),
    ];
    let concurrent = images.clone();
    let (first_result, second_result) = tokio::join!(
        images.replace(item.id, &first),
        concurrent.replace(item.id, &second),
    );
    assert_eq!(first_result.unwrap(), persisted(item.id, &first));
    assert_eq!(second_result.unwrap(), persisted(item.id, &second));

    let final_rows = images.list(item.id).await.unwrap();
    assert!(
        final_rows == persisted(item.id, &first) || final_rows == persisted(item.id, &second),
        "concurrent replace must leave one complete input set: {final_rows:?}"
    );
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT conname, pg_get_constraintdef(oid) AS definition \
             FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.base_item_images'::regclass \
             ORDER BY conname"
                .to_owned(),
        ))
        .await
        .expect("base-item image constraint catalog")
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "conname").unwrap(),
                String::try_get(&row, "", "definition").unwrap(),
            )
        })
        .collect::<BTreeMap<String, String>>();
    for expected in [
        "base_item_images_pkey",
        "base_item_images_item_id_fkey",
        "base_item_images_type_valid",
        "base_item_images_index_nonnegative",
        "base_item_images_path_not_blank",
        "base_item_images_width_positive",
        "base_item_images_height_positive",
    ] {
        assert!(constraints.contains_key(expected), "missing {expected}");
    }
    assert_eq!(
        constraints["base_item_images_pkey"],
        "PRIMARY KEY (item_id, image_type, image_index)"
    );
    assert!(
        constraints["base_item_images_item_id_fkey"].contains("ON DELETE CASCADE"),
        "unexpected FK: {}",
        constraints["base_item_images_item_id_fkey"]
    );

    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname, indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'base_item_images' \
             ORDER BY indexname"
                .to_owned(),
        ))
        .await
        .expect("base-item image index catalog")
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "indexname").unwrap(),
                String::try_get(&row, "", "indexdef").unwrap(),
            )
        })
        .collect::<BTreeMap<String, String>>();
    assert_eq!(
        indexes.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "base_item_images_pkey",
            "base_item_images_primary_lookup_idx"
        ]
    );
    let primary = &indexes["base_item_images_primary_lookup_idx"];
    for fragment in [
        "USING btree (item_id)",
        "INCLUDE (path, date_modified, width, height, blurhash)",
        "image_type = 0",
        "image_index = 0",
    ] {
        assert!(
            primary.contains(fragment),
            "missing {fragment} in {primary}"
        );
    }
}

async fn assert_primary_lookup_plan(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
) {
    let item = create_item(items, "explain").await;
    let mut rows = vec![image(
        BaseItemImageType::Primary,
        0,
        "explain-primary.jpg",
        700,
    )];
    rows.extend((0_u32..256).map(|index| {
        image(
            BaseItemImageType::Backdrop,
            index,
            &format!("explain-backdrop-{index}.jpg"),
            701 + i64::from(index),
        )
    }));
    images.replace(item.id, &rows).await.unwrap();
    database
        .execute_unprepared("ANALYZE jellyfin.base_item_images")
        .await
        .expect("analyze base-item images");

    let transaction = database.begin().await.expect("EXPLAIN transaction");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");
    let plan = transaction
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "EXPLAIN (FORMAT TEXT) \
             SELECT path, date_modified, width, height, blurhash \
             FROM jellyfin.base_item_images \
             WHERE item_id = $1 AND image_type = 0 AND image_index = 0",
            [item.id.into()],
        ))
        .await
        .expect("primary image EXPLAIN")
        .iter()
        .map(explain_line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("base_item_images_primary_lookup_idx"),
        "expected partial primary-image index in plan:\n{plan}"
    );
    transaction.rollback().await.expect("EXPLAIN rollback");
}

fn explain_line(row: &QueryResult) -> String {
    String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line must be text")
}

async fn create_item(items: &BaseItemRepository, label: &str) -> base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Movie");
    item.name = Some(label.to_owned());
    item.sort_name = Some(label.to_owned());
    items.create(item).await.expect("base-item creation")
}

fn image(
    image_type: BaseItemImageType,
    image_index: u32,
    path: &str,
    timestamp_offset: i64,
) -> NewBaseItemImage {
    NewBaseItemImage {
        image_type,
        image_index,
        path: path.to_owned(),
        date_modified: timestamp(1_700_000_000 + timestamp_offset),
        width: Some(1920),
        height: Some(1080),
        blurhash: Some(format!("blur-{timestamp_offset}")),
    }
}

fn persisted(item_id: Uuid, images: &[NewBaseItemImage]) -> Vec<BaseItemImage> {
    let mut rows = images
        .iter()
        .map(|image| BaseItemImage {
            item_id,
            image_type: image.image_type,
            image_index: image.image_index,
            path: image.path.clone(),
            date_modified: image.date_modified,
            width: image.width,
            height: image.height,
            blurhash: image.blurhash.clone(),
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|image| (image.image_type.as_i16(), image.image_index));
    rows
}

fn timestamp(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).expect("test timestamp must be valid")
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
