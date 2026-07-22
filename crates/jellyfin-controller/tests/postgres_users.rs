use jellyfin_controller::{UserError, UserService};
use jellyfin_data::entities::user;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

#[tokio::test]
async fn concurrent_case_insensitive_user_creation_is_serialized() {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let suffix = Uuid::new_v4().simple().to_string();
    let lower_name = format!("concurrent-{suffix}");
    let upper_name = lower_name.to_uppercase();
    let service = UserService::new(database.clone());

    let (first, second) = tokio::join!(service.create(&lower_name), service.create(&upper_name));
    let successes = usize::from(first.is_ok()) + usize::from(second.is_ok());
    assert_eq!(successes, 1, "exactly one concurrent insert must win");

    let failure = if first.is_err() { first } else { second };
    assert!(
        matches!(failure, Err(UserError::DuplicateUsername(_))),
        "unexpected failure: {failure:?}"
    );

    let stored = service
        .get_by_name(&upper_name)
        .await
        .expect("lookup must succeed")
        .expect("winning user must exist");
    assert_eq!(stored.normalized_username, upper_name);

    user::Entity::delete_many()
        .filter(user::Column::Id.eq(stored.id))
        .exec(&database)
        .await
        .expect("test user cleanup must succeed");
}
