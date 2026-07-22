use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use jellyfin_data::{
    DatabaseConfig, NewUserProfileImage, UserProfileImageRepository, UserProfileImageStoreError,
    entities::{user, user_profile_image},
};
use jellyfin_migration::CreateUserProfileImagesMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, PaginatorTrait,
    QueryFilter, Statement, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_profile_images_";

#[tokio::test]
async fn postgres_user_profile_images_are_atomic_and_key_independent() {
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
        exercise_profile_images(&task_database_name).await;
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

async fn exercise_profile_images(database_name: &str) {
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
    CreateUserProfileImagesMigration
        .up(&schema)
        .await
        .expect("reapplying profile-image DDL must succeed");
    CreateUserProfileImagesMigration
        .up(&schema)
        .await
        .expect("profile-image DDL must remain idempotent");
    assert_schema(&database).await;

    let repository = UserProfileImageRepository::new(database.clone());
    assert_detached_input_clear(&database, &repository).await;
    assert_no_image_is_noop(&database, &repository).await;
    let replacement_user = assert_replacement(&database, &repository).await;
    assert_cascade(&database, &repository).await;
    assert_path_constraints(&database, &repository).await;
    assert_concurrent_clear(&repository, replacement_user).await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_detached_input_clear(
    database: &DatabaseConnection,
    repository: &UserProfileImageRepository,
) {
    let user_id = create_user(database, "detached").await;
    let detached = NewUserProfileImage {
        user_id,
        path: format!("/tmp/{user_id}-temporary-profile.png"),
        last_modified: timestamp(1_700_000_000),
    };
    let persisted = repository
        .upsert(detached.clone())
        .await
        .expect("detached profile image must persist");
    let restarted = UserProfileImageRepository::new(database.clone());
    assert_eq!(
        restarted.get(user_id).await.expect("persisted lookup"),
        Some(persisted.clone())
    );

    let removed = restarted
        .clear(detached.user_id)
        .await
        .expect("clear must use only the detached input's user ID");
    assert_eq!(removed, Some(persisted));
    assert_eq!(
        detached.path,
        format!("/tmp/{user_id}-temporary-profile.png")
    );
    assert_eq!(repository.get(user_id).await.unwrap(), None);
}

async fn assert_no_image_is_noop(
    database: &DatabaseConnection,
    repository: &UserProfileImageRepository,
) {
    let user_id = create_user(database, "no-image").await;
    assert_eq!(repository.clear(user_id).await.unwrap(), None);
    assert_eq!(repository.clear(user_id).await.unwrap(), None);
}

async fn assert_replacement(
    database: &DatabaseConnection,
    repository: &UserProfileImageRepository,
) -> Uuid {
    let user_id = create_user(database, "replacement").await;
    let original = repository
        .upsert(image(user_id, "original.png", 1_700_000_100))
        .await
        .expect("original profile image");
    let replacement = repository
        .upsert(image(user_id, "replacement.png", 1_700_000_200))
        .await
        .expect("replacement profile image");

    assert_eq!(replacement.user_id, original.user_id);
    assert_eq!(replacement.path, "replacement.png");
    assert_eq!(replacement.last_modified, timestamp(1_700_000_200));
    assert_ne!(replacement.last_modified, original.last_modified);
    assert_eq!(repository.get(user_id).await.unwrap(), Some(replacement));
    assert_eq!(
        user_profile_image::Entity::find()
            .filter(user_profile_image::Column::UserId.eq(user_id))
            .count(database)
            .await
            .expect("profile image count"),
        1
    );
    user_id
}

async fn assert_cascade(database: &DatabaseConnection, repository: &UserProfileImageRepository) {
    let user_id = create_user(database, "cascade").await;
    repository
        .upsert(image(user_id, "cascade.png", 1_700_000_300))
        .await
        .expect("cascade profile image");
    user::Entity::delete_by_id(user_id)
        .exec(database)
        .await
        .expect("user deletion must succeed");
    assert_eq!(repository.get(user_id).await.unwrap(), None);
}

async fn assert_path_constraints(
    database: &DatabaseConnection,
    repository: &UserProfileImageRepository,
) {
    let user_id = create_user(database, "constraint").await;
    assert!(matches!(
        repository
            .upsert(image(user_id, " \t ", 1_700_000_400))
            .await,
        Err(UserProfileImageStoreError::EmptyPath)
    ));

    let blank = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.user_profile_images (user_id, path, last_modified) \
             VALUES ($1, $2, $3)",
            [
                user_id.into(),
                " \t ".into(),
                timestamp(1_700_000_400).into(),
            ],
        ))
        .await;
    assert!(blank.is_err(), "database CHECK must reject blank paths");

    let null_path = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.user_profile_images (user_id, path, last_modified) \
             VALUES ($1, NULL, $2)",
            [user_id.into(), timestamp(1_700_000_400).into()],
        ))
        .await;
    assert!(null_path.is_err(), "database must reject NULL paths");

    let maximum = "x".repeat(512);
    let maximum_image = repository
        .upsert(NewUserProfileImage {
            user_id,
            path: maximum.clone(),
            last_modified: timestamp(1_700_000_401),
        })
        .await
        .expect("512-character path must be accepted");
    assert_eq!(maximum_image.path, maximum);

    let too_long = "x".repeat(513);
    assert!(matches!(
        repository
            .upsert(NewUserProfileImage {
                user_id,
                path: too_long.clone(),
                last_modified: timestamp(1_700_000_402),
            })
            .await,
        Err(UserProfileImageStoreError::PathTooLong { max: 512 })
    ));
    let oversized_user = create_user(database, "oversized").await;
    let oversized = database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.user_profile_images (user_id, path, last_modified) \
             VALUES ($1, $2, $3)",
            [
                oversized_user.into(),
                too_long.into(),
                timestamp(1_700_000_402).into(),
            ],
        ))
        .await;
    assert!(
        oversized.is_err(),
        "database varchar limit must reject 513 characters"
    );
}

async fn assert_concurrent_clear(repository: &UserProfileImageRepository, user_id: Uuid) {
    let second = repository.clone();
    let (first_result, second_result) =
        tokio::join!(repository.clear(user_id), second.clear(user_id),);
    let removed = [
        first_result.expect("first concurrent clear"),
        second_result.expect("second concurrent clear"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].user_id, user_id);
    assert_eq!(repository.clear(user_id).await.unwrap(), None);
}

async fn assert_schema(database: &DatabaseConnection) {
    let indexes = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'user_profile_images' \
             ORDER BY indexname"
                .to_owned(),
        ))
        .await
        .expect("profile-image index catalog query")
        .into_iter()
        .map(|row| String::try_get(&row, "", "indexname").expect("index name"))
        .collect::<Vec<_>>();
    assert_eq!(indexes, ["user_profile_images_pkey"]);

    let constraints = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT conname, contype::text AS constraint_type, \
                    confdeltype::text AS delete_action \
             FROM pg_constraint \
             WHERE connamespace = 'jellyfin'::regnamespace \
               AND conrelid = 'jellyfin.user_profile_images'::regclass \
             ORDER BY conname"
                .to_owned(),
        ))
        .await
        .expect("profile-image constraint catalog query")
        .into_iter()
        .map(|row| {
            let name = String::try_get(&row, "", "conname").expect("constraint name");
            let kind = String::try_get(&row, "", "constraint_type").expect("constraint type");
            let delete = String::try_get(&row, "", "delete_action").expect("delete action");
            (name, (kind, delete))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        constraints,
        BTreeMap::from([
            (
                "user_profile_images_path_not_blank".to_owned(),
                ("c".to_owned(), " ".to_owned()),
            ),
            (
                "user_profile_images_pkey".to_owned(),
                ("p".to_owned(), " ".to_owned()),
            ),
            (
                "user_profile_images_user_id_fkey".to_owned(),
                ("f".to_owned(), "c".to_owned()),
            ),
        ])
    );

    let columns = database
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT column_name, data_type, character_maximum_length, is_nullable \
             FROM information_schema.columns \
             WHERE table_schema = 'jellyfin' \
               AND table_name = 'user_profile_images' \
               AND column_name IN ('path', 'last_modified') \
             ORDER BY column_name"
                .to_owned(),
        ))
        .await
        .expect("profile-image column catalog query");
    assert_eq!(columns.len(), 2);
    let last_modified = &columns[0];
    assert_eq!(
        String::try_get(last_modified, "", "column_name").unwrap(),
        "last_modified"
    );
    assert_eq!(
        String::try_get(last_modified, "", "data_type").unwrap(),
        "timestamp with time zone"
    );
    assert_eq!(
        String::try_get(last_modified, "", "is_nullable").unwrap(),
        "NO"
    );
    let path = &columns[1];
    assert_eq!(String::try_get(path, "", "column_name").unwrap(), "path");
    assert_eq!(
        String::try_get(path, "", "data_type").unwrap(),
        "character varying"
    );
    assert_eq!(
        i32::try_get(path, "", "character_maximum_length").unwrap(),
        512
    );
    assert_eq!(String::try_get(path, "", "is_nullable").unwrap(), "NO");
}

async fn create_user(database: &DatabaseConnection, label: &str) -> Uuid {
    let id = Uuid::new_v4();
    let suffix = id.simple().to_string();
    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.users (id, username, normalized_username) VALUES ($1, $2, $3)",
            [
                id.into(),
                format!("profile-{label}-{suffix}").into(),
                format!("PROFILE-{label}-{suffix}").into(),
            ],
        ))
        .await
        .expect("profile-image test user must insert");
    id
}

fn image(user_id: Uuid, path: &str, last_modified: i64) -> NewUserProfileImage {
    NewUserProfileImage {
        user_id,
        path: path.to_owned(),
        last_modified: timestamp(last_modified),
    }
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
