use chrono::{Duration, Utc};
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceQuery, DeviceRepository, NewDevice,
    entities::{device, user},
};
use jellyfin_migration::CreateAuthenticationMigration;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, Set, SqlErr, Statement, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_authentication_vertical_slice() {
    let database = prepare_database().await;
    test_api_keys(&database).await;
    test_devices(&database).await;
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    CreateAuthenticationMigration
        .up(&schema)
        .await
        .expect("reapplying authentication DDL must succeed");
    CreateAuthenticationMigration
        .up(&schema)
        .await
        .expect("authentication DDL must remain idempotent");
    assert_authentication_indexes(&database).await;
    database
}

async fn test_api_keys(database: &DatabaseConnection) {
    let repository = ApiKeyRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let key = repository
        .create(&format!("test-{suffix}"))
        .await
        .expect("API key must be created");
    assert_eq!(key.access_token.len(), 32);

    let found = repository
        .find_by_token(&key.access_token)
        .await
        .expect("API key lookup must succeed")
        .expect("API key must exist");
    assert_eq!(found, key);
    assert!(
        repository
            .list()
            .await
            .expect("API key list must load")
            .contains(&key)
    );

    let touched_at = key.date_last_activity + Duration::seconds(5);
    assert_eq!(
        repository
            .touch(&key.access_token, touched_at)
            .await
            .expect("API key touch must succeed"),
        1
    );
    let touched = repository
        .find_by_token(&key.access_token)
        .await
        .expect("touched API key lookup must succeed")
        .expect("touched API key must exist");
    assert_eq!(touched.date_last_activity, touched_at);

    let mut duplicate = key.clone().into_active_model();
    duplicate.id = NotSet;
    duplicate.name = Set(format!("duplicate-{suffix}"));
    let error = duplicate
        .insert(database)
        .await
        .expect_err("duplicate API token must be rejected");
    assert!(matches!(
        error.sql_err(),
        Some(SqlErr::UniqueConstraintViolation(_))
    ));

    assert_eq!(
        repository
            .revoke(&key.access_token)
            .await
            .expect("API key revoke must succeed"),
        1
    );
    assert!(
        repository
            .find_by_token(&key.access_token)
            .await
            .expect("revoked API key lookup must succeed")
            .is_none()
    );
}

async fn test_devices(database: &DatabaseConnection) {
    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = Uuid::new_v4();
    insert_user(database, user_id, &format!("DeviceUser-{suffix}")).await;
    let repository = DeviceRepository::new(database.clone());
    let device_id = format!("device-{suffix}");

    let first = repository
        .create(NewDevice::new(
            user_id,
            "Jellyfin Web",
            "1.0",
            "Browser",
            &device_id,
        ))
        .await
        .expect("first device token must be created");
    let second = repository
        .create(NewDevice::new(
            user_id,
            "Jellyfin Mobile",
            "",
            "",
            &device_id,
        ))
        .await
        .expect("second device token must be created");
    assert_ne!(first.access_token, second.access_token);

    let page = repository
        .query(&DeviceQuery {
            user_id: Some(user_id),
            device_id: Some(device_id.clone()),
            limit: Some(1),
            ..Default::default()
        })
        .await
        .expect("device query must succeed");
    assert_eq!(page.total_record_count, 2);
    assert_eq!(page.items.len(), 1);

    let token_page = repository
        .query(&DeviceQuery {
            access_token: Some(second.access_token.clone()),
            ..Default::default()
        })
        .await
        .expect("device token query must succeed");
    assert_eq!(token_page.items.len(), 1);
    assert_eq!(token_page.items[0].id, second.id);

    let mut activated = first.clone();
    activated.is_active = true;
    activated.date_last_activity = Utc::now() + Duration::seconds(5);
    let activated = repository
        .update(activated)
        .await
        .expect("device update must succeed");
    let latest = repository
        .latest_by_device_id(&device_id)
        .await
        .expect("latest device lookup must succeed")
        .expect("latest device must exist");
    assert_eq!(latest.id, activated.id);

    let mut duplicate = second.clone().into_active_model();
    duplicate.id = NotSet;
    duplicate.device_id = Set(format!("other-{suffix}"));
    let error = duplicate
        .insert(database)
        .await
        .expect_err("duplicate device token must be rejected");
    assert!(matches!(
        error.sql_err(),
        Some(SqlErr::UniqueConstraintViolation(_))
    ));

    assert_eq!(
        repository
            .delete(second.id)
            .await
            .expect("device deletion must succeed"),
        1
    );
    user::Entity::delete_by_id(user_id)
        .exec(database)
        .await
        .expect("device test user deletion must succeed");
    assert!(
        device::Entity::find_by_id(activated.id)
            .one(database)
            .await
            .expect("cascade verification must succeed")
            .is_none()
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
        .expect("device test user must be inserted");
}

async fn assert_authentication_indexes(database: &DatabaseConnection) {
    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT tablename, indexname, indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename IN ('api_keys', 'devices')"
                .to_owned(),
        ))
        .await
        .expect("authentication index catalog query must succeed");
    let indexes: Vec<(String, String, String)> = rows
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "tablename").expect("table name must be text"),
                String::try_get(&row, "", "indexname").expect("index name must be text"),
                String::try_get(&row, "", "indexdef").expect("index definition must be text"),
            )
        })
        .collect();

    assert_unique_index(&indexes, "api_keys", "api_keys_access_token_key");
    assert_unique_index(&indexes, "devices", "devices_access_token_key");
    assert!(indexes.iter().any(|(table, name, definition)| {
        table == "devices"
            && name == "devices_device_activity_idx"
            && definition.contains("device_id, date_last_activity DESC")
    }));
    assert!(
        indexes
            .iter()
            .any(|(table, name, _)| { table == "devices" && name == "devices_user_device_idx" })
    );
}

fn assert_unique_index(indexes: &[(String, String, String)], table: &str, name: &str) {
    assert!(indexes.iter().any(|(found_table, found_name, definition)| {
        found_table == table && found_name == name && definition.contains("CREATE UNIQUE INDEX")
    }));
}
