use jellyfin_data::{
    BaseItemError, BaseItemRepository, DatabaseConfig, NewBaseItem,
    entities::{ancestor_id, base_item},
};
use jellyfin_migration::CreateBaseItemsMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, EntityTrait, QueryResult, Statement, TransactionTrait,
    TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn postgres_base_items_vertical_slice() {
    let database = prepare_database().await;
    let repository = BaseItemRepository::new(database.clone());

    assert_placeholder_and_validation(&repository).await;
    assert_crud_hierarchy_and_move(&database, &repository).await;
    assert_concurrent_hierarchy_mutations(&repository).await;
    assert_postgres_catalog(&database).await;
    assert_postgres_query_plans(&database, &repository).await;
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreateBaseItemsMigration
        .up(&schema)
        .await
        .expect("reapplying base-item DDL must succeed");
    CreateBaseItemsMigration
        .up(&schema)
        .await
        .expect("base-item DDL must remain idempotent");
    database
}

async fn assert_placeholder_and_validation(repository: &BaseItemRepository) {
    let placeholder = repository
        .get(Uuid::from_u128(1))
        .await
        .expect("placeholder lookup must succeed")
        .expect("official detached user-data placeholder must exist");
    assert_eq!(placeholder.item_type, "PLACEHOLDER");

    let invalid = repository
        .create(NewBaseItem::new(Uuid::new_v4(), "  "))
        .await;
    assert!(matches!(invalid, Err(BaseItemError::InvalidItemType)));

    let id = Uuid::new_v4();
    let mut self_parent = NewBaseItem::new(id, "Folder");
    self_parent.parent_id = Some(id);
    let error = repository.create(self_parent).await;
    assert!(matches!(error, Err(BaseItemError::HierarchyCycle)));

    let mut missing_parent = NewBaseItem::new(Uuid::new_v4(), "Movie");
    missing_parent.parent_id = Some(Uuid::new_v4());
    let error = repository.create(missing_parent).await;
    assert!(matches!(error, Err(BaseItemError::ParentNotFound)));
}

async fn assert_crud_hierarchy_and_move(
    database: &DatabaseConnection,
    repository: &BaseItemRepository,
) {
    let left = create_item(repository, "Folder", "Left", None, true).await;
    let right = create_item(repository, "Folder", "Right", None, true).await;
    let zulu = create_item(repository, "Folder", "Zulu", Some(left.id), true).await;
    let alpha = create_item(repository, "Movie", "Alpha", Some(left.id), false).await;
    let grandchild = create_item(repository, "Episode", "Grand", Some(zulu.id), false).await;

    assert_lookup_and_ordering(repository, &left, &zulu, &alpha).await;
    assert_closure(repository, left.id, zulu.id, grandchild.id).await;
    assert_optimistic_update(repository, alpha).await;
    assert_move_and_cycles(repository, &left, &right, &zulu, &grandchild).await;

    assert!(repository.delete(right.id).await.expect("subtree delete"));
    assert!(!repository.exists(right.id).await.expect("right existence"));
    assert!(!repository.exists(zulu.id).await.expect("child existence"));
    assert!(
        !repository
            .exists(grandchild.id)
            .await
            .expect("grandchild existence")
    );
    assert!(
        ancestor_id::Entity::find_by_id((grandchild.id, right.id))
            .one(database)
            .await
            .expect("deleted closure lookup")
            .is_none()
    );
    assert!(repository.delete(left.id).await.expect("left cleanup"));
}

async fn assert_lookup_and_ordering(
    repository: &BaseItemRepository,
    root: &base_item::Model,
    zulu: &base_item::Model,
    alpha: &base_item::Model,
) {
    assert!(repository.exists(root.id).await.expect("root existence"));
    assert!(
        repository
            .exists_by_path(root.path.as_deref().expect("root path"))
            .await
            .expect("path existence")
    );
    let loaded = repository
        .get(root.id)
        .await
        .expect("root lookup")
        .expect("root must exist");
    assert_eq!(loaded.data, Some(json!({ "source": "integration-test" })));
    let children = repository.children(root.id).await.expect("children lookup");
    assert_eq!(
        children.iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![alpha.id, zulu.id]
    );
    let parent = repository
        .parent(zulu.id)
        .await
        .expect("parent lookup")
        .expect("zulu must have a parent");
    assert_eq!(parent.id, root.id);
}

async fn assert_closure(
    repository: &BaseItemRepository,
    root_id: Uuid,
    child_id: Uuid,
    grandchild_id: Uuid,
) {
    let ancestors = repository
        .ancestors(grandchild_id)
        .await
        .expect("ancestor lookup");
    assert_eq!(
        ancestors
            .iter()
            .map(|entry| (entry.item.id, entry.depth))
            .collect::<Vec<_>>(),
        vec![(child_id, 1), (root_id, 2)]
    );
    let descendants = repository
        .descendants(root_id)
        .await
        .expect("descendant lookup");
    assert_eq!(descendants.len(), 3);
    assert!(
        descendants
            .iter()
            .any(|entry| entry.item.id == grandchild_id && entry.depth == 2)
    );
    let scan_candidates = repository
        .descendant_scan_candidates(root_id)
        .await
        .expect("scan candidate lookup");
    assert_eq!(
        scan_candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>(),
        descendants
            .iter()
            .map(|entry| entry.item.id)
            .collect::<Vec<_>>()
    );
    let grandchild_candidate = scan_candidates
        .iter()
        .find(|candidate| candidate.id == grandchild_id)
        .expect("grandchild scan candidate");
    assert_eq!(grandchild_candidate.item_type, "Episode");
    assert_eq!(grandchild_candidate.parent_id, Some(child_id));
    assert_eq!(
        grandchild_candidate.path.as_deref(),
        Some(grandchild_id.to_string().as_str())
    );
    let grandchild = repository
        .get(grandchild_id)
        .await
        .expect("grandchild lookup")
        .expect("grandchild must exist");
    assert_eq!(grandchild.top_parent_id, Some(root_id));
}

async fn assert_optimistic_update(repository: &BaseItemRepository, original: base_item::Model) {
    let stale = original.clone();
    let previous_modified = original.date_modified;
    let mut changed = original;
    changed.name = Some("Updated Alpha".to_owned());
    let updated = repository.update(changed).await.expect("item update");
    assert_eq!(updated.row_version, stale.row_version + 1);
    assert!(updated.date_modified >= previous_modified);
    assert_eq!(updated.name.as_deref(), Some("Updated Alpha"));
    let error = repository.update(stale).await;
    assert!(matches!(error, Err(BaseItemError::StaleVersion)));
}

async fn assert_move_and_cycles(
    repository: &BaseItemRepository,
    left: &base_item::Model,
    right: &base_item::Model,
    child: &base_item::Model,
    grandchild: &base_item::Model,
) {
    let moved = repository
        .move_item(child.id, Some(right.id), child.row_version)
        .await
        .expect("subtree move");
    assert_eq!(moved.top_parent_id, Some(right.id));
    let refreshed_grandchild = repository
        .get(grandchild.id)
        .await
        .expect("moved grandchild lookup")
        .expect("moved grandchild must exist");
    assert_eq!(refreshed_grandchild.top_parent_id, Some(right.id));
    let old_descendants = repository
        .descendants(left.id)
        .await
        .expect("old subtree lookup");
    assert!(
        old_descendants
            .iter()
            .all(|entry| entry.item.id != child.id && entry.item.id != grandchild.id)
    );
    let ancestors = repository
        .ancestors(grandchild.id)
        .await
        .expect("moved ancestor lookup");
    assert_eq!(
        ancestors
            .iter()
            .map(|entry| (entry.item.id, entry.depth))
            .collect::<Vec<_>>(),
        vec![(child.id, 1), (right.id, 2)]
    );

    let error = repository
        .move_item(right.id, Some(grandchild.id), right.row_version)
        .await;
    assert!(matches!(error, Err(BaseItemError::HierarchyCycle)));
}

async fn assert_concurrent_hierarchy_mutations(repository: &BaseItemRepository) {
    let root = create_item(repository, "Folder", "Concurrent", None, true).await;
    let mut first = NewBaseItem::new(Uuid::new_v4(), "Movie");
    first.parent_id = Some(root.id);
    first.sort_name = Some("First".to_owned());
    let mut second = NewBaseItem::new(Uuid::new_v4(), "Movie");
    second.parent_id = Some(root.id);
    second.sort_name = Some("Second".to_owned());
    let (first_result, second_result) =
        tokio::join!(repository.create(first), repository.create(second));
    first_result.expect("first concurrent child");
    second_result.expect("second concurrent child");
    assert_eq!(
        repository.children(root.id).await.expect("children").len(),
        2
    );
    repository
        .delete(root.id)
        .await
        .expect("concurrent cleanup");

    let first_root = create_item(repository, "Folder", "Move-A", None, true).await;
    let second_root = create_item(repository, "Folder", "Move-B", None, true).await;
    let (first_move, second_move) = tokio::join!(
        repository.move_item(first_root.id, Some(second_root.id), first_root.row_version),
        repository.move_item(second_root.id, Some(first_root.id), second_root.row_version),
    );
    assert_eq!(
        usize::from(first_move.is_ok()) + usize::from(second_move.is_ok()),
        1
    );
    assert!(matches!(
        first_move.as_ref().err().or(second_move.as_ref().err()),
        Some(BaseItemError::HierarchyCycle)
    ));
    let top = if first_move.is_ok() {
        second_root.id
    } else {
        first_root.id
    };
    repository.delete(top).await.expect("opposing move cleanup");
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = catalog_names(
        database,
        "SELECT conname AS name FROM pg_constraint \
         WHERE conrelid IN ('jellyfin.base_items'::regclass, 'jellyfin.ancestor_ids'::regclass)",
    )
    .await;
    for expected in [
        "base_items_parent_id_fkey",
        "base_items_parent_not_self",
        "ancestor_ids_one_parent_per_depth",
        "ancestor_ids_depth_positive",
    ] {
        assert!(constraints.iter().any(|name| name == expected));
    }
    let triggers = catalog_names(
        database,
        "SELECT tgname AS name FROM pg_trigger \
         WHERE tgrelid = 'jellyfin.base_items'::regclass AND NOT tgisinternal",
    )
    .await;
    for expected in [
        "base_items_validate_insert",
        "base_items_validate_parent_update",
        "base_items_rebuild_insert",
        "base_items_rebuild_parent_update",
        "base_items_touch_row_version",
    ] {
        assert!(triggers.iter().any(|name| name == expected));
    }
    let indexes = catalog_names(
        database,
        "SELECT indexname AS name FROM pg_indexes \
         WHERE schemaname = 'jellyfin' \
           AND tablename IN ('base_items', 'ancestor_ids')",
    )
    .await;
    for expected in expected_index_names() {
        assert!(indexes.iter().any(|name| name == expected));
    }
}

async fn catalog_names(database: &DatabaseConnection, sql: &str) -> Vec<String> {
    database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            sql.to_owned(),
        ))
        .await
        .expect("PostgreSQL catalog query")
        .into_iter()
        .map(|row| String::try_get(&row, "", "name").expect("catalog name must be text"))
        .collect()
}

async fn assert_postgres_query_plans(
    database: &DatabaseConnection,
    repository: &BaseItemRepository,
) {
    let root = create_item(repository, "Folder", "Explain", None, true).await;
    let child = create_item(repository, "Movie", "Explain Child", Some(root.id), false).await;
    let transaction = database.begin().await.expect("explain transaction");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");
    let queries = [
        (
            &["base_items_parent_sort_idx"][..],
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.base_items \
             WHERE parent_id = $1 ORDER BY sort_name, id",
            root.id,
        ),
        (
            &["base_items_path_hash_idx"][..],
            "EXPLAIN (FORMAT TEXT) SELECT 1 FROM jellyfin.base_items WHERE path = $1::uuid::text",
            root.id,
        ),
        (
            &[
                "ancestor_ids_item_depth_idx",
                "ancestor_ids_one_parent_per_depth",
            ][..],
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.ancestor_ids \
             WHERE item_id = $1 ORDER BY depth",
            child.id,
        ),
        (
            &["ancestor_ids_parent_depth_idx"][..],
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.ancestor_ids \
             WHERE parent_item_id = $1 ORDER BY depth, item_id",
            root.id,
        ),
    ];
    for (indexes, sql, id) in queries {
        let plan = explain(&transaction, sql, id).await;
        assert!(
            indexes.iter().any(|index| plan.contains(index)),
            "expected one of {indexes:?} in plan:\n{plan}"
        );
    }
    transaction.rollback().await.expect("explain rollback");
    repository.delete(root.id).await.expect("explain cleanup");
}

async fn explain(transaction: &sea_orm::DatabaseTransaction, sql: &str, id: Uuid) -> String {
    transaction
        .query_all(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            sql,
            [id.into()],
        ))
        .await
        .expect("EXPLAIN query")
        .iter()
        .map(explain_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn explain_line(row: &QueryResult) -> String {
    String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line must be text")
}

fn expected_index_names() -> [&'static str; 5] {
    [
        "base_items_parent_sort_idx",
        "base_items_path_hash_idx",
        "base_items_top_type_sort_idx",
        "ancestor_ids_item_depth_idx",
        "ancestor_ids_parent_depth_idx",
    ]
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    sort_name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
) -> base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, item_type);
    item.parent_id = parent_id;
    item.name = Some(sort_name.to_owned());
    item.sort_name = Some(sort_name.to_owned());
    item.path = Some(id.to_string());
    item.data = Some(json!({ "source": "integration-test" }));
    item.is_folder = is_folder;
    repository.create(item).await.expect("item creation")
}
