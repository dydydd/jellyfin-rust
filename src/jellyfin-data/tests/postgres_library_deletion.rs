use jellyfin_data::{
    BaseItemError, BaseItemRepository, DatabaseConfig, NewBaseItem, NewUserData,
    UserDataRepository,
    entities::{
        ancestor_id, base_item, item_value, item_value_map, person, person_base_item_map, user,
        user_data,
    },
};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, Statement,
};
use uuid::Uuid;

#[tokio::test]
async fn postgres_library_deletion_is_atomic_and_cascades_complete_subtrees() {
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = Uuid::new_v4();
    insert_user(&database, user_id, &format!("LibraryDelete-{suffix}")).await;
    let items = BaseItemRepository::new(database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let parent = create_item(&items, "Folder", "Delete Parent", root.id).await;
    let child = create_item(&items, "Movie", "Delete Child", parent.id).await;
    let grandchild = create_item(&items, "Episode", "Delete Grandchild", child.id).await;
    let independent = create_item(&items, "Movie", "Delete Independent", root.id).await;
    let survivor = create_item(&items, "Movie", "Delete Survivor", root.id).await;

    let value_id = Uuid::new_v4();
    let person_id = Uuid::new_v4();
    insert_item_maps(&database, value_id, person_id, child.id, grandchild.id).await;
    let detached_item_id = Uuid::new_v4();
    let user_data_repository = UserDataRepository::new(database.clone());
    for item_id in [
        parent.id,
        child.id,
        grandchild.id,
        independent.id,
        detached_item_id,
    ] {
        user_data_repository
            .upsert(NewUserData::new(item_id, user_id, "library-delete"))
            .await
            .expect("library deletion user data");
    }

    let missing = Uuid::new_v4();
    assert!(matches!(
        items.delete_many(&[survivor.id, missing]).await,
        Err(BaseItemError::NotFound)
    ));
    assert!(items.exists(survivor.id).await.expect("atomic survivor"));
    assert!(matches!(
        items.delete_many(&[root.id]).await,
        Err(BaseItemError::ProtectedItem)
    ));

    items
        .delete_many(&[parent.id, independent.id])
        .await
        .expect("batch subtree deletion");
    for item_id in [parent.id, child.id, grandchild.id, independent.id] {
        assert!(!items.exists(item_id).await.expect("deleted item lookup"));
    }
    assert!(items.exists(survivor.id).await.expect("unrelated survivor"));
    assert_cascades(
        &database,
        user_id,
        detached_item_id,
        &[parent.id, child.id, grandchild.id, independent.id],
        value_id,
        person_id,
    )
    .await;

    items.delete(survivor.id).await.expect("survivor cleanup");
    user::Entity::delete_by_id(user_id)
        .exec(&database)
        .await
        .expect("library deletion user cleanup");
    item_value::Entity::delete_by_id(value_id)
        .exec(&database)
        .await
        .expect("item value cleanup");
    person::Entity::delete_by_id(person_id)
        .exec(&database)
        .await
        .expect("person cleanup");
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    database
}

async fn assert_cascades(
    database: &DatabaseConnection,
    user_id: Uuid,
    detached_item_id: Uuid,
    deleted_item_ids: &[Uuid],
    value_id: Uuid,
    person_id: Uuid,
) {
    assert_eq!(
        ancestor_id::Entity::find()
            .filter(
                Condition::any()
                    .add(ancestor_id::Column::ItemId.is_in(deleted_item_ids.iter().copied()))
                    .add(
                        ancestor_id::Column::ParentItemId
                            .is_in(deleted_item_ids.iter().copied()),
                    ),
            )
            .count(database)
            .await
            .expect("deleted closure count"),
        0
    );
    assert_eq!(
        item_value_map::Entity::find()
            .filter(item_value_map::Column::ItemValueId.eq(value_id))
            .count(database)
            .await
            .expect("deleted item value map count"),
        0
    );
    assert_eq!(
        person_base_item_map::Entity::find()
            .filter(person_base_item_map::Column::PersonId.eq(person_id))
            .count(database)
            .await
            .unwrap_or_default(),
        0
    );
    assert_eq!(
        user_data::Entity::find()
            .filter(user_data::Column::UserId.eq(user_id))
            .filter(user_data::Column::ItemId.ne(detached_item_id))
            .count(database)
            .await
            .expect("deleted user-data count"),
        0
    );
    assert!(
        user_data::Entity::find_by_id((detached_item_id, user_id, "library-delete".to_owned()))
            .one(database)
            .await
            .expect("detached user data lookup")
            .is_some()
    );
    assert!(
        item_value::Entity::find_by_id(value_id)
            .one(database)
            .await
            .expect("shared item value lookup")
            .is_some()
    );
    assert!(
        person::Entity::find_by_id(person_id)
            .one(database)
            .await
            .expect("shared person lookup")
            .is_some()
    );
}

async fn insert_user(database: &DatabaseConnection, user_id: Uuid, username: &str) {
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.users (id, username, normalized_username) VALUES ($1, $2, $3)",
            [
                user_id.into(),
                username.into(),
                username.to_uppercase().into(),
            ],
        ))
        .await
        .expect("library deletion user insert");
}

async fn insert_item_maps(
    database: &DatabaseConnection,
    value_id: Uuid,
    person_id: Uuid,
    value_item_id: Uuid,
    person_item_id: Uuid,
) {
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.item_values (item_value_id, type, value, clean_value) \
             VALUES ($1, 0, $2, $3)",
            [
                value_id.into(),
                format!("Value-{value_id}").into(),
                format!("value-{value_id}").into(),
            ],
        ))
        .await
        .expect("library deletion item value");
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.item_value_map (item_value_id, item_id) VALUES ($1, $2)",
            [value_id.into(), value_item_id.into()],
        ))
        .await
        .expect("library deletion item value map");
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.people (id, name, clean_name) VALUES ($1, $2, $3)",
            [
                person_id.into(),
                format!("Person-{person_id}").into(),
                format!("person-{person_id}").into(),
            ],
        ))
        .await
        .expect("library deletion person");
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.people_base_item_map \
                 (item_id, person_id, person_type, list_order) VALUES ($1, $2, 'Actor', 0)",
            [person_item_id.into(), person_id.into()],
        ))
        .await
        .expect("library deletion person map");
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    repository
        .create(item)
        .await
        .expect("library deletion item")
}
