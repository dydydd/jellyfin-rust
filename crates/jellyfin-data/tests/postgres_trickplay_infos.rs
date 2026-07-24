use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, LinkedChildType, NewBaseItem, NewTrickplayInfo,
    TrickplayInfoRepository, TrickplayInfoStoreError,
    entities::{base_item, linked_child, trickplay_info},
};
use jellyfin_migration::{CreateTrickplayInfosMigration, OptimizeTrickplayManifestsMigration};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    QueryFilter, Statement, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_trickplay_infos_";

#[tokio::test]
async fn postgres_trickplay_infos_are_constrained_upserted_and_cascaded() {
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
        exercise_trickplay_infos(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_trickplay_infos(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 6,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    CreateTrickplayInfosMigration
        .up(&schema)
        .await
        .expect("reapplying trickplay DDL must succeed");
    OptimizeTrickplayManifestsMigration
        .up(&schema)
        .await
        .expect("reapplying trickplay manifest indexes must succeed");

    let items = BaseItemRepository::new(database.clone());
    let repository = TrickplayInfoRepository::new(database.clone());
    let item = create_item(&items).await;

    assert_upsert_and_resolution_key(&database, &repository, item.id).await;
    assert_batch_manifests_expand_all_local_sources(&database, &items, &repository, item.id).await;
    assert_validation_and_missing_owner(&repository).await;
    assert_database_constraints(&database, item.id).await;
    assert_catalog(&database).await;
    assert_manifest_indexes(&database).await;
    assert_cascade(&items, &database, item).await;

    database.close().await.unwrap();
}

async fn assert_manifest_indexes(database: &DatabaseConnection) {
    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT indexname, indexdef
            FROM pg_indexes
            WHERE schemaname = 'jellyfin'
              AND indexname IN (
                  'linked_children_alternate_child_lookup_idx',
                  'trickplay_infos_manifest_covering_idx'
              )
            "
            .to_owned(),
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "indexname").unwrap(),
                String::try_get(&row, "", "indexdef").unwrap(),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert!(
        indexes["linked_children_alternate_child_lookup_idx"]
            .contains("(child_id, parent_id) WHERE (child_type = ANY (ARRAY[2, 3]))")
    );
    let covering = &indexes["trickplay_infos_manifest_covering_idx"];
    assert!(covering.contains("(item_id, width) INCLUDE"));
    for column in [
        "height",
        "tile_width",
        "tile_height",
        "thumbnail_count",
        "interval",
        "bandwidth",
    ] {
        assert!(covering.contains(column));
    }
}

async fn assert_batch_manifests_expand_all_local_sources(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    repository: &TrickplayInfoRepository,
    primary_id: Uuid,
) {
    let alternate = create_item(items).await;
    let linked = create_item(items).await;
    let empty = create_item(items).await;
    let group_primary = items
        .merge_alternate_versions(&[primary_id, alternate.id])
        .await
        .unwrap();
    linked_child::Entity::insert(linked_child::ActiveModel {
        parent_id: Set(group_primary),
        child_id: Set(linked.id),
        child_type: Set(LinkedChildType::LinkedAlternateVersion as i16),
        sort_order: Set(None),
    })
    .exec(database)
    .await
    .unwrap();
    repository
        .upsert(alternate.id, info(480, 270, 3, 2, 10, 1_000, 33_000))
        .await
        .unwrap();
    repository
        .upsert(linked.id, info(720, 405, 4, 3, 20, 750, 55_000))
        .await
        .unwrap();

    let manifests = repository
        .manifests_for_items(&[primary_id, alternate.id, empty.id, primary_id])
        .await
        .unwrap();
    for display_id in [primary_id, alternate.id] {
        let manifest = &manifests[&display_id];
        assert_eq!(manifest[&primary_id].len(), 2);
        assert_eq!(manifest[&alternate.id][&480].height, 270);
        assert_eq!(manifest[&linked.id][&720].bandwidth, 55_000);
    }
    assert!(manifests[&empty.id].is_empty());
    assert!(
        repository
            .manifests_for_items(&[])
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_upsert_and_resolution_key(
    database: &DatabaseConnection,
    repository: &TrickplayInfoRepository,
    item_id: Uuid,
) {
    let initial = info(320, 180, 2, 2, 6, 1_500, 22_000);
    let inserted = repository.upsert(item_id, initial).await.unwrap();
    assert_eq!(inserted.item_id, item_id);
    assert_eq!(inserted.width, 320);
    assert_eq!(repository.get(item_id, 320).await.unwrap(), Some(inserted));

    let updated = info(320, 181, 4, 3, 17, 750, 44_000);
    let stored = repository.upsert(item_id, updated).await.unwrap();
    assert_eq!(stored.height, 181);
    assert_eq!(stored.tile_width, 4);
    assert_eq!(stored.thumbnail_count, 17);

    let alternate = repository
        .upsert(item_id, info(640, 360, 5, 5, 25, 1_000, 80_000))
        .await
        .unwrap();
    assert_eq!(alternate.width, 640);
    assert_eq!(
        trickplay_info::Entity::find()
            .filter(trickplay_info::Column::ItemId.eq(item_id))
            .all(database)
            .await
            .unwrap()
            .len(),
        2
    );
}

async fn assert_validation_and_missing_owner(repository: &TrickplayInfoRepository) {
    assert!(matches!(
        repository
            .upsert(Uuid::new_v4(), info(320, 180, 2, 2, 6, 1_500, 22_000))
            .await,
        Err(TrickplayInfoStoreError::BaseItemNotFound { .. })
    ));

    for invalid in [
        info(0, 180, 2, 2, 6, 1_500, 22_000),
        info(320, 0, 2, 2, 6, 1_500, 22_000),
        info(320, 180, 0, 2, 6, 1_500, 22_000),
        info(320, 180, 2, 0, 6, 1_500, 22_000),
        info(320, 180, 2, 2, -1, 1_500, 22_000),
        info(320, 180, 2, 2, 6, 0, 22_000),
        info(320, 180, 2, 2, 6, 1_500, -1),
    ] {
        assert!(matches!(
            repository.upsert(Uuid::new_v4(), invalid).await,
            Err(TrickplayInfoStoreError::InvalidValue { .. })
        ));
    }
}

async fn assert_database_constraints(database: &DatabaseConnection, item_id: Uuid) {
    let error = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.trickplay_infos (
                item_id, width, height, tile_width, tile_height,
                thumbnail_count, interval, bandwidth
            ) VALUES ($1, 999, 180, 0, 2, 6, 1500, 22000)
            ",
            [item_id.into()],
        ))
        .await
        .expect_err("database must reject a zero tile width");
    assert!(
        error
            .to_string()
            .contains("trickplay_infos_tile_width_positive")
    );
}

async fn assert_catalog(database: &DatabaseConnection) {
    let rows = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT conname, pg_get_constraintdef(oid) AS definition
            FROM pg_constraint
            WHERE conrelid = 'jellyfin.trickplay_infos'::regclass
            ORDER BY conname
            "
            .to_owned(),
        ))
        .await
        .unwrap();
    let constraints = rows
        .iter()
        .map(|row| {
            (
                String::try_get(row, "", "conname").unwrap(),
                String::try_get(row, "", "definition").unwrap(),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        constraints["trickplay_infos_pkey"],
        "PRIMARY KEY (item_id, width)"
    );
    assert!(
        constraints["trickplay_infos_item_id_fkey"]
            .contains("FOREIGN KEY (item_id) REFERENCES jellyfin.base_items(id) ON DELETE CASCADE")
    );
    assert_eq!(constraints.len(), 9);
}

async fn assert_cascade(
    items: &BaseItemRepository,
    database: &DatabaseConnection,
    item: base_item::Model,
) {
    let item_id = item.id;
    items.delete(item_id).await.unwrap();
    assert!(
        trickplay_info::Entity::find()
            .filter(trickplay_info::Column::ItemId.eq(item_id))
            .all(database)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn create_item(repository: &BaseItemRepository) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Video");
    item.name = Some("Trickplay Test Video".to_owned());
    item.media_type = Some("Video".to_owned());
    item.path = Some("/media/trickplay-test.mkv".to_owned());
    repository.create(item).await.unwrap()
}

const fn info(
    width: i32,
    height: i32,
    tile_width: i32,
    tile_height: i32,
    thumbnail_count: i32,
    interval: i32,
    bandwidth: i32,
) -> NewTrickplayInfo {
    NewTrickplayInfo {
        width,
        height,
        tile_width,
        tile_height,
        thumbnail_count,
        interval,
        bandwidth,
    }
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
