use std::collections::{HashMap, HashSet};

use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, LinkedChildRepository, LinkedChildStoreError,
    LinkedChildType, NewBaseItem,
    entities::{base_item, linked_child},
};
use jellyfin_migration::CreateLinkedChildrenMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, ModelTrait,
    QueryFilter, Statement, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_linked_children_";

#[tokio::test]
async fn postgres_linked_children_are_ordered_constrained_and_concurrent() {
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
        exercise_linked_children(&task_database_name).await;
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

async fn exercise_linked_children(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    CreateLinkedChildrenMigration
        .up(&schema)
        .await
        .expect("reapplying linked-child DDL must succeed");
    CreateLinkedChildrenMigration
        .up(&schema)
        .await
        .expect("linked-child DDL must remain idempotent");

    let items = BaseItemRepository::new(database.clone());
    let links = LinkedChildRepository::new(database.clone());
    assert_order_idempotency_and_errors(&database, &items, &links).await;
    assert_concurrent_appends(&items, &links).await;
    assert_foreign_keys_and_repository_deletion(&database, &items, &links).await;
    assert_database_constraints(&database, &items).await;
    assert_catalog(&database).await;

    database.close().await.unwrap();
}

async fn assert_order_idempotency_and_errors(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    links: &LinkedChildRepository,
) {
    let parent = create_item(items, "BoxSet", "Ordered collection").await;
    let first = create_item(items, "Movie", "First").await;
    let second = create_item(items, "Movie", "Second").await;
    let third = create_item(items, "Movie", "Third").await;

    let stored = links
        .add_manual(parent.id, &[second.id, first.id, second.id])
        .await
        .unwrap();
    assert_eq!(child_ids(&stored), [second.id, first.id]);
    assert_eq!(sort_orders(&stored), [Some(0), Some(1)]);
    assert!(
        stored.iter().all(|link| {
            link.parent_id == parent.id && link.child_type == LinkedChildType::Manual
        })
    );

    let stored = links
        .add_manual(parent.id, &[first.id, third.id])
        .await
        .unwrap();
    assert_eq!(child_ids(&stored), [second.id, first.id, third.id]);
    assert_eq!(sort_orders(&stored), [Some(0), Some(1), Some(2)]);

    let stored = links
        .remove(parent.id, &[Uuid::new_v4(), first.id, first.id])
        .await
        .unwrap();
    assert_eq!(child_ids(&stored), [second.id, third.id]);
    assert_eq!(sort_orders(&stored), [Some(0), Some(2)]);
    assert_eq!(links.remove(parent.id, &[]).await.unwrap(), stored);

    let unordered = create_item(items, "Movie", "Unordered shortcut").await;
    linked_child::Entity::insert(linked_child::ActiveModel {
        parent_id: sea_orm::Set(parent.id),
        child_id: sea_orm::Set(unordered.id),
        child_type: sea_orm::Set(LinkedChildType::Shortcut as i16),
        sort_order: sea_orm::Set(None),
    })
    .exec(database)
    .await
    .unwrap();
    assert_eq!(
        links
            .list(parent.id)
            .await
            .unwrap()
            .last()
            .unwrap()
            .child_id,
        unordered.id
    );

    assert!(matches!(
        links.add_manual(Uuid::new_v4(), &[first.id]).await,
        Err(LinkedChildStoreError::ParentNotFound { .. })
    ));
    assert!(matches!(
        links.add_manual(parent.id, &[Uuid::new_v4()]).await,
        Err(LinkedChildStoreError::ChildNotFound { .. })
    ));
    assert!(matches!(
        links.add_manual(parent.id, &[parent.id]).await,
        Err(LinkedChildStoreError::SelfLink)
    ));
    assert!(matches!(
        links.remove(Uuid::new_v4(), &[first.id]).await,
        Err(LinkedChildStoreError::ParentNotFound { .. })
    ));
}

async fn assert_concurrent_appends(items: &BaseItemRepository, links: &LinkedChildRepository) {
    let parent = create_item(items, "BoxSet", "Concurrent collection").await;
    let mut children = Vec::new();
    for index in 0..12 {
        children.push(create_item(items, "Movie", &format!("Concurrent {index}")).await);
    }

    let mut tasks = Vec::new();
    for chunk in children.chunks(3) {
        let repository = links.clone();
        let child_ids = chunk.iter().map(|item| item.id).collect::<Vec<_>>();
        tasks.push(tokio::spawn(async move {
            repository.add_manual(parent.id, &child_ids).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let stored = links.list(parent.id).await.unwrap();
    assert_eq!(stored.len(), children.len());
    assert_eq!(
        child_ids(&stored).into_iter().collect::<HashSet<_>>(),
        children.iter().map(|item| item.id).collect()
    );
    assert_eq!(
        sort_orders(&stored),
        (0..i32::try_from(children.len()).unwrap())
            .map(Some)
            .collect::<Vec<_>>()
    );
}

async fn assert_foreign_keys_and_repository_deletion(
    database: &DatabaseConnection,
    items: &BaseItemRepository,
    links: &LinkedChildRepository,
) {
    let parent = create_item(items, "BoxSet", "Delete parent").await;
    let child = create_item(items, "Movie", "Delete child").await;
    links.add_manual(parent.id, &[child.id]).await.unwrap();

    let error = child
        .clone()
        .delete(database)
        .await
        .expect_err("NO ACTION must reject deleting a directly referenced item");
    assert!(error.to_string().contains("linked_children_child_id_fkey"));

    items.delete_many(&[child.id]).await.unwrap();
    assert!(links.list(parent.id).await.unwrap().is_empty());
    assert!(items.get(child.id).await.unwrap().is_none());

    let child = create_item(items, "Movie", "Delete with parent").await;
    links.add_manual(parent.id, &[child.id]).await.unwrap();
    items.delete_many(&[parent.id]).await.unwrap();
    assert!(items.get(parent.id).await.unwrap().is_none());
    assert!(
        linked_child::Entity::find()
            .filter(linked_child::Column::ParentId.eq(parent.id))
            .all(database)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn assert_database_constraints(database: &DatabaseConnection, items: &BaseItemRepository) {
    let parent = create_item(items, "BoxSet", "Constraints").await;
    let child = create_item(items, "Movie", "Constraint child").await;
    for (child_type, sort_order, child_id, constraint) in [
        (4_i16, Some(0_i32), child.id, "linked_children_type_valid"),
        (
            LinkedChildType::Manual as i16,
            Some(-1),
            child.id,
            "linked_children_sort_order_nonnegative",
        ),
        (
            LinkedChildType::Manual as i16,
            Some(0),
            parent.id,
            "linked_children_not_self",
        ),
    ] {
        let error = database
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                INSERT INTO jellyfin.linked_children
                    (parent_id, child_id, child_type, sort_order)
                VALUES ($1, $2, $3, $4)
                ",
                [
                    parent.id.into(),
                    child_id.into(),
                    child_type.into(),
                    sort_order.into(),
                ],
            ))
            .await
            .expect_err("linked-child check constraint must reject invalid data");
        assert!(error.to_string().contains(constraint));
    }
}

async fn assert_catalog(database: &DatabaseConnection) {
    let rows = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT conname, pg_get_constraintdef(oid) AS definition
            FROM pg_constraint
            WHERE conrelid = 'jellyfin.linked_children'::regclass
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
        .collect::<HashMap<_, _>>();
    assert_eq!(
        constraints["linked_children_pkey"],
        "PRIMARY KEY (parent_id, child_id)"
    );
    assert!(
        constraints["linked_children_parent_id_fkey"]
            .contains("FOREIGN KEY (parent_id) REFERENCES jellyfin.base_items(id)")
    );
    assert!(
        constraints["linked_children_child_id_fkey"]
            .contains("FOREIGN KEY (child_id) REFERENCES jellyfin.base_items(id)")
    );
    assert_eq!(constraints.len(), 6);

    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT indexname, indexdef
            FROM pg_indexes
            WHERE schemaname = 'jellyfin' AND tablename = 'linked_children'
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
        .collect::<HashMap<_, _>>();
    assert!(
        indexes["linked_children_parent_order_idx"]
            .contains("(parent_id, sort_order, child_id) INCLUDE (child_type)")
    );
    assert!(indexes["linked_children_manual_parent_lookup_idx"].contains("WHERE (child_type = 0)"));
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.is_folder = item_type == "BoxSet";
    repository.create(item).await.unwrap()
}

fn child_ids(links: &[jellyfin_data::LinkedChild]) -> Vec<Uuid> {
    links.iter().map(|link| link.child_id).collect()
}

fn sort_orders(links: &[jellyfin_data::LinkedChild]) -> Vec<Option<i32>> {
    links.iter().map(|link| link.sort_order).collect()
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
