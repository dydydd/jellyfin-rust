use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, EmbyItemAccessLevel, EmbyItemAccessRepository,
    EmbyItemAccessStoreError, NewBaseItem, entities::emby_item_access,
};
use jellyfin_migration::CreateEmbyItemAccessMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Statement,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_emby_item_access_is_atomic_set_based_and_cascading() {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreateEmbyItemAccessMigration
        .up(&schema)
        .await
        .expect("reapplying Emby item-access DDL must succeed");

    let suffix = Uuid::new_v4().simple().to_string();
    let first_user = Uuid::new_v4();
    let second_user = Uuid::new_v4();
    insert_user(&database, first_user, &format!("access-first-{suffix}")).await;
    insert_user(&database, second_user, &format!("access-second-{suffix}")).await;
    let items = BaseItemRepository::new(database.clone());
    let first_item = Uuid::new_v4();
    let second_item = Uuid::new_v4();
    items
        .create(NewBaseItem::new(first_item, "Movie"))
        .await
        .expect("first item");
    items
        .create(NewBaseItem::new(second_item, "Episode"))
        .await
        .expect("second item");

    let repository = EmbyItemAccessRepository::new(database.clone());
    repository
        .replace(
            &[first_user, second_user, first_user],
            &[first_item, second_item, first_item],
            Some(EmbyItemAccessLevel::Read),
        )
        .await
        .expect("deduplicated Cartesian-product insert");
    assert_eq!(
        rows_for_users(&database, &[first_user, second_user]).await,
        4
    );
    assert_eq!(stored(&repository, first_user, first_item).await, Some(1));

    repository
        .replace(
            &[first_user],
            &[first_item, second_item],
            Some(EmbyItemAccessLevel::ManageDelete),
        )
        .await
        .expect("replace levels");
    assert_eq!(stored(&repository, first_user, first_item).await, Some(4));
    assert_eq!(stored(&repository, first_user, second_item).await, Some(4));
    assert_eq!(stored(&repository, second_user, first_item).await, Some(1));

    let error = repository
        .replace(
            &[first_user, Uuid::new_v4()],
            &[first_item],
            Some(EmbyItemAccessLevel::Write),
        )
        .await
        .expect_err("missing user must abort");
    assert!(matches!(error, EmbyItemAccessStoreError::UserNotFound));
    assert_eq!(stored(&repository, first_user, first_item).await, Some(4));

    let error = repository
        .replace(
            &[first_user],
            &[first_item, Uuid::new_v4()],
            Some(EmbyItemAccessLevel::Manage),
        )
        .await
        .expect_err("missing item must abort");
    assert!(matches!(error, EmbyItemAccessStoreError::ItemNotFound));
    assert_eq!(stored(&repository, first_user, first_item).await, Some(4));

    repository
        .replace(&[first_user], &[first_item], None)
        .await
        .expect("None deletes explicit row");
    assert_eq!(stored(&repository, first_user, first_item).await, None);

    delete_user(&database, second_user).await;
    assert_eq!(stored(&repository, second_user, first_item).await, None);
    items.delete(second_item).await.expect("delete second item");
    assert_eq!(stored(&repository, first_user, second_item).await, None);

    delete_user(&database, first_user).await;
    items.delete(first_item).await.expect("delete first item");
    database.close().await.expect("database cleanup");
}

async fn stored(
    repository: &EmbyItemAccessRepository,
    user_id: Uuid,
    item_id: Uuid,
) -> Option<i16> {
    repository
        .get(user_id, item_id)
        .await
        .expect("item-access lookup")
        .map(|row| row.access_level)
}

async fn rows_for_users(database: &DatabaseConnection, user_ids: &[Uuid]) -> u64 {
    emby_item_access::Entity::find()
        .filter(emby_item_access::Column::UserId.is_in(user_ids.iter().copied()))
        .count(database)
        .await
        .expect("item-access count")
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
        .expect("test user insert");
}

async fn delete_user(database: &DatabaseConnection, user_id: Uuid) {
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "DELETE FROM jellyfin.users WHERE id = $1",
            [user_id.into()],
        ))
        .await
        .expect("test user delete");
}
