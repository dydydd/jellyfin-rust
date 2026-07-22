use jellyfin_controller::{UserError, UserService};
use jellyfin_data::{DatabaseConfig, entities::user};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_norm_user_";

#[tokio::test]
async fn normalized_usernames_match_the_official_database_contract() {
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
        exercise_official_matrix(&task_database_name).await;
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

async fn exercise_official_matrix(database_name: &str) {
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
    let users = UserService::new(database.clone());

    assert_non_ascii_normalized_lookup(&database, &users).await;
    assert_case_insensitive_lookup(&database, &users).await;
    assert_missing_lookup(&users).await;
    assert_create_collisions(&database, &users).await;
    assert_distinct_non_ascii_names(&database, &users).await;
    assert_rename_normalization(&database, &users).await;
    assert_rename_collisions(&database, &users).await;

    drop(users);
    database.close().await.expect("database pool must close");
}

async fn assert_non_ascii_normalized_lookup(database: &DatabaseConnection, users: &UserService) {
    for (username, normalized_lookup) in [
        ("münchen", "MÜNCHEN"),
        ("Ñoño", "ÑOÑO"),
        ("jellyfin", "JELLYFIN"),
        ("Çelebi", "ÇELEBI"),
    ] {
        let created = users.create(username).await.expect("user creation");
        let found = users
            .get_by_name(normalized_lookup)
            .await
            .expect("normalized lookup")
            .expect("normalized user");
        assert_eq!(found.id, created.id, "username={username:?}");
        assert_eq!(found.username, username);
        delete_users(database, &[created.id]).await;
    }
}

async fn assert_case_insensitive_lookup(database: &DatabaseConnection, users: &UserService) {
    for username in ["münchen", "Ñoño", "ali", "testüser"] {
        let created = users.create(username).await.expect("user creation");
        for lookup in [
            username.to_uppercase(),
            username.to_lowercase(),
            username.to_owned(),
        ] {
            let found = users
                .get_by_name(&lookup)
                .await
                .expect("case-insensitive lookup")
                .expect("case-insensitive user");
            assert_eq!(
                found.id, created.id,
                "username={username:?}, lookup={lookup:?}"
            );
        }
        delete_users(database, &[created.id]).await;
    }
}

async fn assert_missing_lookup(users: &UserService) {
    for lookup in ["nonexistent", "MÜNCHEN"] {
        assert!(
            users
                .get_by_name(lookup)
                .await
                .expect("missing lookup must succeed")
                .is_none(),
            "lookup={lookup:?}"
        );
    }
}

async fn assert_create_collisions(database: &DatabaseConnection, users: &UserService) {
    for (existing_username, duplicate_username) in [
        ("münchen", "MÜNCHEN"),
        ("Ñoño", "ñoño"),
        ("alice", "ALICE"),
        ("çelebi", "ÇELEBI"),
    ] {
        let existing = users
            .create(existing_username)
            .await
            .expect("existing user creation");
        assert!(matches!(
            users.create(duplicate_username).await,
            Err(UserError::DuplicateUsername(name)) if name == duplicate_username
        ));
        delete_users(database, &[existing.id]).await;
    }
}

async fn assert_distinct_non_ascii_names(database: &DatabaseConnection, users: &UserService) {
    for (first_username, second_username) in
        [("münchen", "münchen2"), ("ali", "ali2"), ("noño", "nono")]
    {
        let first = users
            .create(first_username)
            .await
            .expect("first distinct user creation");
        let second = users
            .create(second_username)
            .await
            .expect("second distinct user creation");
        assert_ne!(first.id, second.id);
        assert_ne!(first.normalized_username, second.normalized_username);
        delete_users(database, &[first.id, second.id]).await;
    }
}

async fn assert_rename_normalization(database: &DatabaseConnection, users: &UserService) {
    for (original_name, new_name) in [
        ("alice", "münchen"),
        ("müller", "mueller"),
        ("ali", "ALI2"),
        ("testuser", "Ñoño"),
    ] {
        let created = users
            .create(original_name)
            .await
            .expect("rename target creation");
        let renamed = users
            .rename(created.id, new_name)
            .await
            .expect("user rename");
        assert_eq!(renamed.username, new_name);
        assert_eq!(renamed.normalized_username, new_name.to_uppercase());
        assert!(
            users
                .get_by_name(original_name)
                .await
                .expect("old-name lookup")
                .is_none()
        );
        assert_eq!(
            users
                .get_by_name(new_name)
                .await
                .expect("new-name lookup")
                .expect("renamed user")
                .id,
            created.id
        );
        delete_users(database, &[created.id]).await;
    }
}

async fn assert_rename_collisions(database: &DatabaseConnection, users: &UserService) {
    for (existing_username, conflicting_new_name) in [
        ("münchen", "MÜNCHEN"),
        ("Ñoño", "ñoño"),
        ("alice", "Alice"),
        ("testüser", "TESTÜSER"),
    ] {
        let target = users
            .create("renametarget")
            .await
            .expect("rename target creation");
        let existing = users
            .create(existing_username)
            .await
            .expect("conflicting user creation");
        assert!(matches!(
            users.rename(target.id, conflicting_new_name).await,
            Err(UserError::DuplicateUsername(name)) if name == conflicting_new_name
        ));
        let unchanged = users.get(target.id).await.expect("unchanged rename target");
        assert_eq!(unchanged.username, "renametarget");
        assert_eq!(unchanged.normalized_username, "RENAMETARGET");
        delete_users(database, &[target.id, existing.id]).await;
    }
}

async fn delete_users(database: &DatabaseConnection, ids: &[Uuid]) {
    user::Entity::delete_many()
        .filter(user::Column::Id.is_in(ids.iter().copied()))
        .exec(database)
        .await
        .expect("matrix users must be deleted");
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database name must have its fixed prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
