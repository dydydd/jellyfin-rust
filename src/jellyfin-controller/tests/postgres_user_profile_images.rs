use chrono::{DateTime, Utc};
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, NewUserProfileImage};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_user_images_";

#[tokio::test]
async fn user_service_clears_persisted_profile_images_by_user_id() {
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
        exercise_user_profile_images(&task_database_name).await;
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

async fn exercise_user_profile_images(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let service = UserService::new(database.clone());
    let user = service
        .create("profile-image-user")
        .await
        .expect("profile-image user creation");
    let persisted = service
        .set_profile_image(image(user.id, "persisted-profile.png", 1_700_000_000))
        .await
        .expect("profile image persistence");

    let restarted = UserService::new(database.clone());
    assert_eq!(
        restarted
            .profile_image(user.id)
            .await
            .expect("profile image lookup"),
        Some(persisted.clone())
    );

    // The caller only supplies the user identity. No detached or temporary
    // image key participates in selection of the database row to delete.
    assert_eq!(
        restarted
            .clear_profile_image(user.id)
            .await
            .expect("profile image clear"),
        Some(persisted)
    );
    assert_eq!(service.profile_image(user.id).await.unwrap(), None);
    assert_eq!(service.clear_profile_image(user.id).await.unwrap(), None);

    let user_without_image = service
        .create("user-without-profile-image")
        .await
        .expect("user without image creation");
    assert_eq!(
        service
            .clear_profile_image(user_without_image.id)
            .await
            .unwrap(),
        None
    );

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

fn image(user_id: Uuid, path: &str, last_modified: i64) -> NewUserProfileImage {
    NewUserProfileImage {
        user_id,
        path: path.to_owned(),
        last_modified: DateTime::<Utc>::from_timestamp(last_modified, 0)
            .expect("test timestamp must be valid"),
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
