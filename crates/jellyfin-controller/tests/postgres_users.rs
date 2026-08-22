use jellyfin_controller::{UserError, UserService};
use jellyfin_data::entities::user;
use jellyfin_model::UserPolicy;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
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

#[tokio::test]
async fn failed_login_attempts_increment_and_lockout_disables_non_administrators() {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let service = UserService::new(database.clone());
    let name = format!("lockout-{}", Uuid::new_v4().simple());
    let user = service
        .create(&name)
        .await
        .expect("user creation must succeed");
    service
        .update_policy(
            user.id,
            &UserPolicy {
                login_attempts_before_lockout: 3,
                invalid_login_attempt_count: 0,
                authentication_provider_id: Some(
                    UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
                ),
                password_reset_provider_id: Some(
                    UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
                ),
                ..UserPolicy::default()
            },
        )
        .await
        .expect("lockout policy update must succeed");

    let first = service
        .record_failed_authentication(user.id)
        .await
        .expect("first failed attempt");
    assert_eq!(first.policy["InvalidLoginAttemptCount"], 1);
    assert!(!first.is_disabled);

    let second = service
        .record_failed_authentication(user.id)
        .await
        .expect("second failed attempt");
    assert_eq!(second.policy["InvalidLoginAttemptCount"], 2);
    assert!(!second.is_disabled);

    let third = service
        .record_failed_authentication(user.id)
        .await
        .expect("third failed attempt");
    assert_eq!(third.policy["InvalidLoginAttemptCount"], 3);
    assert!(third.is_disabled);
    assert_eq!(third.policy["IsDisabled"], true);

    user::Entity::delete_by_id(user.id)
        .exec(&database)
        .await
        .expect("test user cleanup must succeed");
}

#[tokio::test]
async fn policy_updates_persist_canonical_provider_ids_and_projected_flags() {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let service = UserService::new(database.clone());
    let name = format!("policy-{}", Uuid::new_v4().simple());
    let created = service
        .create(&name)
        .await
        .expect("user creation must succeed");

    assert_eq!(
        created.authentication_provider_id,
        UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID
    );
    assert_eq!(
        created.password_reset_provider_id,
        UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID
    );
    assert_eq!(
        created.policy["AuthenticationProviderId"],
        UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID
    );
    assert_eq!(
        created.policy["PasswordResetProviderId"],
        UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID
    );

    let policy = UserPolicy {
        is_hidden: false,
        enable_collection_management: true,
        authentication_provider_id: Some("Example.Authentication, Assembly".to_owned()),
        password_reset_provider_id: Some("Example.PasswordReset, Assembly".to_owned()),
        ..UserPolicy::default()
    };
    let (updated, became_disabled) = service
        .update_policy(created.id, &policy)
        .await
        .expect("arbitrary nonblank provider identifiers must persist");

    assert!(!became_disabled);
    assert_eq!(
        updated.authentication_provider_id,
        "Example.Authentication, Assembly"
    );
    assert_eq!(
        updated.password_reset_provider_id,
        "Example.PasswordReset, Assembly"
    );
    assert_eq!(updated.policy, serde_json::to_value(&policy).unwrap());
    assert!(!updated.is_hidden);
    assert!(!updated.is_disabled);
    assert!(!updated.is_administrator);
    assert!(updated.row_version > created.row_version);

    user::Entity::delete_by_id(created.id)
        .exec(&database)
        .await
        .expect("test user cleanup must succeed");
}

#[tokio::test]
async fn policy_updates_validate_providers_and_forbid_disabling_administrators() {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let service = UserService::new(database.clone());
    let prefix = Uuid::new_v4().simple().to_string();
    let ordinary = service
        .create(&format!("policy-validation-{prefix}"))
        .await
        .expect("ordinary user creation must succeed");
    let administrator = service
        .create_initial_administrator(&format!("policy-admin-{prefix}"))
        .await
        .expect("administrator creation must succeed");

    let valid = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    for invalid in [None, Some(String::new()), Some(" \t".to_owned())] {
        let mut policy = valid.clone();
        policy.authentication_provider_id = invalid.clone();
        assert!(matches!(
            service.update_policy(ordinary.id, &policy).await,
            Err(UserError::InvalidPolicy)
        ));

        let mut policy = valid.clone();
        policy.password_reset_provider_id = invalid;
        assert!(matches!(
            service.update_policy(ordinary.id, &policy).await,
            Err(UserError::InvalidPolicy)
        ));
    }
    let mut overlong = valid.clone();
    overlong.authentication_provider_id = Some("x".repeat(256));
    assert!(matches!(
        service.update_policy(ordinary.id, &overlong).await,
        Err(UserError::InvalidPolicy)
    ));
    let mut overlong = valid.clone();
    overlong.password_reset_provider_id = Some("x".repeat(256));
    assert!(matches!(
        service.update_policy(ordinary.id, &overlong).await,
        Err(UserError::InvalidPolicy)
    ));

    let disabled_administrator = UserPolicy {
        is_administrator: false,
        is_disabled: true,
        ..valid
    };
    assert!(matches!(
        service
            .update_policy(administrator.id, &disabled_administrator)
            .await,
        Err(UserError::AdministratorCannotBeDisabled)
    ));

    user::Entity::delete_many()
        .filter(user::Column::Id.is_in([ordinary.id, administrator.id]))
        .exec(&database)
        .await
        .expect("test user cleanup must succeed");
}

#[tokio::test]
async fn concurrent_policy_updates_preserve_global_user_invariants() {
    let administrator = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("jellyfin_policy_{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name, "jellyfin_policy_");
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");
    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_concurrent_policy_invariants(&task_database_name).await;
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

async fn exercise_concurrent_policy_invariants(database_name: &str) {
    let isolated = jellyfin_data::connect(&jellyfin_data::DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&isolated)
        .await
        .expect("temporary database migrations must succeed");

    let service = UserService::new(isolated.clone());
    let first_admin = service
        .create_initial_administrator("concurrent-admin-one")
        .await
        .unwrap();
    let second_admin = service
        .create_initial_administrator("concurrent-admin-two")
        .await
        .unwrap();
    let demoted = valid_policy();
    let (first, second) = tokio::join!(
        service.update_policy(first_admin.id, &demoted),
        service.update_policy(second_admin.id, &demoted)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = if first.is_err() { first } else { second };
    assert!(matches!(failure, Err(UserError::LastAdministrator)));
    assert_eq!(
        user::Entity::find()
            .filter(user::Column::IsAdministrator.eq(true))
            .count(&isolated)
            .await
            .unwrap(),
        1
    );

    user::Entity::delete_many().exec(&isolated).await.unwrap();
    let first_user = service.create("concurrent-user-one").await.unwrap();
    let second_user = service.create("concurrent-user-two").await.unwrap();
    let mut disabled = valid_policy();
    disabled.is_disabled = true;
    let (first, second) = tokio::join!(
        service.update_policy(first_user.id, &disabled),
        service.update_policy(second_user.id, &disabled)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let failure = if first.is_err() { first } else { second };
    assert!(matches!(failure, Err(UserError::LastEnabledUser)));
    assert_eq!(
        user::Entity::find()
            .filter(user::Column::IsDisabled.eq(false))
            .count(&isolated)
            .await
            .unwrap(),
        1
    );

    drop(service);
    isolated
        .close()
        .await
        .expect("temporary database connection must close");
}

fn assert_temporary_database_name(name: &str, prefix: &str) {
    let suffix = name
        .strip_prefix(prefix)
        .expect("temporary database name must have its fixed prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

fn valid_policy() -> UserPolicy {
    UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}
