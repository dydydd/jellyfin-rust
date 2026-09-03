use chrono::{Duration, Utc};
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceOptionsRepository, DeviceQuery, DeviceRepository,
    NewDevice, NewSessionCommand, SessionCommandRepository,
    entities::{device, device_option, session_command, user},
};
use jellyfin_migration::{
    AddDeviceCapabilitiesMigration, AddSessionAdditionalUsersMigration,
    AddSessionNowViewingMigration, AddSessionPlaybackStateMigration, CreateAuthenticationMigration,
    CreateDeviceOptionsMigration, CreateSessionCommandOutboxMigration,
    OptimizeDeviceSessionQueriesMigration,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, Set, SqlErr, Statement, TransactionTrait, TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
async fn postgres_authentication_vertical_slice() {
    let database = prepare_database().await;
    test_api_keys(&database).await;
    test_devices(&database).await;
    test_device_options(&database).await;
    test_session_command_outbox(&database).await;
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
    OptimizeDeviceSessionQueriesMigration
        .up(&schema)
        .await
        .expect("device session optimization DDL must remain idempotent");
    AddDeviceCapabilitiesMigration
        .up(&schema)
        .await
        .expect("device capabilities DDL must remain idempotent");
    CreateDeviceOptionsMigration
        .up(&schema)
        .await
        .expect("device options DDL must remain idempotent");
    CreateDeviceOptionsMigration
        .up(&schema)
        .await
        .expect("device options DDL must stay idempotent");
    CreateSessionCommandOutboxMigration
        .up(&schema)
        .await
        .expect("session command outbox DDL must remain idempotent");
    CreateSessionCommandOutboxMigration
        .up(&schema)
        .await
        .expect("session command outbox DDL must stay idempotent");
    AddSessionNowViewingMigration
        .up(&schema)
        .await
        .expect("session now-viewing DDL must remain idempotent");
    AddSessionNowViewingMigration
        .up(&schema)
        .await
        .expect("session now-viewing DDL must stay idempotent");
    AddSessionAdditionalUsersMigration
        .up(&schema)
        .await
        .expect("session additional-users DDL must remain idempotent");
    AddSessionAdditionalUsersMigration
        .up(&schema)
        .await
        .expect("session additional-users DDL must stay idempotent");
    AddSessionPlaybackStateMigration
        .up(&schema)
        .await
        .expect("session playback-state DDL must remain idempotent");
    AddSessionPlaybackStateMigration
        .up(&schema)
        .await
        .expect("session playback-state DDL must stay idempotent");
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
    assert_device_query_filters(&repository, &device_id, &activated, &second).await;
    assert_playback_state_update(database, &repository, activated.id).await;
    assert_now_viewing_item_update(database, &repository, activated.id).await;
    assert_additional_users_update(database, &repository, activated.id).await;

    let latest = repository
        .latest_by_device_id(&device_id)
        .await
        .expect("latest device lookup must succeed")
        .expect("latest device must exist");
    assert_eq!(latest.id, activated.id);
    assert_device_session_index_plan(database, &device_id).await;

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
            .delete_by_token(&second.access_token)
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

async fn assert_additional_users_update(
    database: &DatabaseConnection,
    repository: &DeviceRepository,
    device_id: i64,
) {
    let additional_user_id = Uuid::new_v4();
    assert_eq!(
        repository
            .add_additional_user(device_id, additional_user_id, "Additional User")
            .await
            .expect("additional user must be added"),
        1
    );
    assert_eq!(
        repository
            .add_additional_user(device_id, additional_user_id, "Ignored Duplicate")
            .await
            .expect("duplicate additional user add must be idempotent"),
        1
    );
    let updated = device::Entity::find_by_id(device_id)
        .one(database)
        .await
        .expect("device with additional user must load")
        .expect("device with additional user must exist");
    assert_eq!(
        updated.additional_users,
        json!([{
            "UserId": additional_user_id.simple().to_string(),
            "UserName": "Additional User"
        }])
    );

    assert_eq!(
        repository
            .remove_additional_user(device_id, additional_user_id)
            .await
            .expect("additional user must be removed"),
        1
    );
    assert_eq!(
        repository
            .remove_additional_user(device_id, additional_user_id)
            .await
            .expect("missing additional user removal must be idempotent"),
        1
    );
    let cleared = device::Entity::find_by_id(device_id)
        .one(database)
        .await
        .expect("device with removed additional user must load")
        .expect("device with removed additional user must exist");
    assert_eq!(cleared.additional_users, json!([]));
}

async fn assert_playback_state_update(
    database: &DatabaseConnection,
    repository: &DeviceRepository,
    device_id: i64,
) {
    assert_eq!(
        repository
            .update_playback_state(
                device_id,
                json!({
                    "PositionTicks": 123,
                    "CanSeek": true,
                    "IsPaused": true
                }),
                Some(json!({
                    "Name": "Now Playing",
                    "Id": Uuid::new_v4().simple().to_string(),
                    "Type": "Movie"
                })),
                json!([{
                    "Id": "queue-1"
                }]),
                Some("playlist-1".to_owned()),
                true,
            )
            .await
            .expect("playback state update must succeed"),
        1
    );
    let updated = device::Entity::find_by_id(device_id)
        .one(database)
        .await
        .expect("device with playback state must load")
        .expect("device with playback state must exist");
    assert_eq!(updated.play_state["PositionTicks"], 123);
    assert_eq!(
        updated.now_playing_item.as_ref().unwrap()["Name"],
        "Now Playing"
    );
    assert_eq!(updated.now_playing_queue[0]["Id"], "queue-1");
    assert_eq!(updated.playlist_item_id.as_deref(), Some("playlist-1"));
    assert!(
        updated.date_last_paused.is_some(),
        "paused playback should remember the first paused timestamp"
    );

    let error = repository
        .update_playback_state(device_id, Value::Null, None, json!([]), None, false)
        .await
        .expect_err("non-object play state must be rejected");
    assert!(matches!(
        error,
        jellyfin_data::AuthenticationStoreError::InvalidPlayState
    ));

    assert_eq!(
        repository
            .clear_playback_state(device_id)
            .await
            .expect("playback state clear must succeed"),
        1
    );
    let cleared = device::Entity::find_by_id(device_id)
        .one(database)
        .await
        .expect("device with cleared playback state must load")
        .expect("device with cleared playback state must exist");
    assert_eq!(cleared.play_state, json!({}));
    assert!(cleared.now_playing_item.is_none());
    assert_eq!(cleared.now_playing_queue, json!([]));
    assert!(cleared.playlist_item_id.is_none());
    assert!(cleared.date_last_paused.is_none());
}

async fn assert_now_viewing_item_update(
    database: &DatabaseConnection,
    repository: &DeviceRepository,
    device_id: i64,
) {
    assert_eq!(
        repository
            .update_now_viewing_item(
                device_id,
                Some(json!({
                    "Name": "The Matrix",
                    "Id": Uuid::new_v4().simple().to_string(),
                    "Type": "Movie"
                })),
            )
            .await
            .expect("now-viewing update must succeed"),
        1
    );
    let updated = device::Entity::find_by_id(device_id)
        .one(database)
        .await
        .expect("updated device must load")
        .expect("updated device must exist");
    assert_eq!(
        updated.now_viewing_item.as_ref().unwrap()["Name"],
        "The Matrix"
    );

    let error = repository
        .update_now_viewing_item(device_id, Some(Value::Null))
        .await
        .expect_err("non-object now-viewing payload must be rejected");
    assert!(matches!(
        error,
        jellyfin_data::AuthenticationStoreError::InvalidNowViewingItem
    ));
}

async fn assert_device_query_filters(
    repository: &DeviceRepository,
    device_id: &str,
    active: &device::Model,
    inactive: &device::Model,
) {
    let active_page = repository
        .query(&DeviceQuery {
            device_id: Some(device_id.to_uppercase()),
            is_active: Some(true),
            active_since: Some(active.date_last_activity - Duration::seconds(1)),
            ..Default::default()
        })
        .await
        .expect("active device query must succeed");
    assert_eq!(active_page.total_record_count, 1);
    assert_eq!(active_page.items[0].id, active.id);

    let stale_page = repository
        .query(&DeviceQuery {
            device_id: Some(device_id.to_uppercase()),
            is_active: Some(true),
            active_since: Some(active.date_last_activity + Duration::seconds(1)),
            ..Default::default()
        })
        .await
        .expect("stale device query must succeed");
    assert_eq!(stale_page.total_record_count, 0);

    let inactive_page = repository
        .query(&DeviceQuery {
            device_id: Some(device_id.to_uppercase()),
            is_active: Some(false),
            ..Default::default()
        })
        .await
        .expect("inactive device query must succeed");
    assert_eq!(inactive_page.total_record_count, 1);
    assert_eq!(inactive_page.items[0].id, inactive.id);
}

async fn test_device_options(database: &DatabaseConnection) {
    let repository = DeviceOptionsRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let device_id = format!("option-device-{suffix}");
    assert!(
        repository
            .get(&device_id)
            .await
            .expect("missing device options lookup must succeed")
            .is_none()
    );

    let created = repository
        .upsert_custom_name(&device_id, Some("Living Room".to_owned()))
        .await
        .expect("device options must be created");
    assert_eq!(created.device_id, device_id);
    assert_eq!(created.custom_name.as_deref(), Some("Living Room"));

    let updated = repository
        .upsert_custom_name(&device_id, Some("Bedroom".to_owned()))
        .await
        .expect("device options must be updated");
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.custom_name.as_deref(), Some("Bedroom"));

    let cleared = repository
        .upsert_custom_name(&device_id, None)
        .await
        .expect("device options custom name must be nullable");
    assert_eq!(cleared.id, created.id);
    assert_eq!(cleared.custom_name, None);

    let requested_ids = [device_id.clone(), format!("missing-{suffix}")];
    let found = repository
        .find_by_device_ids(requested_ids.iter().map(String::as_str))
        .await
        .expect("device options batch lookup must succeed");
    assert_eq!(found, vec![cleared.clone()]);

    let mut duplicate = cleared.into_active_model();
    duplicate.id = NotSet;
    duplicate.custom_name = Set(Some("Duplicate".to_owned()));
    let error = duplicate
        .insert(database)
        .await
        .expect_err("duplicate device options device id must be rejected");
    assert!(matches!(
        error.sql_err(),
        Some(SqlErr::UniqueConstraintViolation(_))
    ));

    device_option::Entity::delete_by_id(created.id)
        .exec(database)
        .await
        .expect("device options cleanup must succeed");
}

async fn test_session_command_outbox(database: &DatabaseConnection) {
    let repository = SessionCommandRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();
    let target_session_id = format!("target-session-{suffix}");
    let command = repository
        .enqueue(NewSessionCommand {
            target_session_id: target_session_id.clone(),
            controlling_session_id: Some(format!("controller-{suffix}")),
            message_type: "GeneralCommand".to_owned(),
            payload: json!({
                "Name": "DisplayMessage",
                "Arguments": {
                    "Header": "Message from Server",
                    "Text": "Hello"
                }
            }),
        })
        .await
        .expect("session command must be queued");
    assert_eq!(command.target_session_id, target_session_id);
    assert_eq!(command.message_type, "GeneralCommand");
    assert_eq!(command.payload["Arguments"]["Text"], "Hello");

    let commands = repository
        .list_for_session(&command.target_session_id)
        .await
        .expect("session command lookup must succeed");
    assert_eq!(commands, vec![command.clone()]);
    assert_eq!(
        repository
            .delete(&[command.id])
            .await
            .expect("session command deletion must succeed"),
        1
    );
    assert!(
        repository
            .list_for_session(&command.target_session_id)
            .await
            .expect("session command lookup must succeed")
            .is_empty()
    );

    let error = repository
        .enqueue(NewSessionCommand {
            target_session_id: format!("invalid-{suffix}"),
            controlling_session_id: None,
            message_type: "GeneralCommand".to_owned(),
            payload: Value::Null,
        })
        .await
        .expect_err("non-object payload must be rejected before insertion");
    assert!(matches!(
        error,
        jellyfin_data::SessionCommandStoreError::InvalidPayload
    ));

    session_command::Entity::delete_by_id(command.id)
        .exec(database)
        .await
        .expect("session command cleanup must succeed");
}

async fn assert_device_session_index_plan(database: &DatabaseConnection, device_id: &str) {
    let transaction = database.begin().await.expect("EXPLAIN transaction");
    transaction
        .execute_unprepared("ANALYZE jellyfin.devices; SET LOCAL enable_seqscan = off")
        .await
        .expect("device session planner statistics must refresh");
    let row = transaction
        .query_one(Statement::from_sql_and_values(
            transaction.get_database_backend(),
            "EXPLAIN (FORMAT JSON) SELECT id FROM jellyfin.devices \
             WHERE is_active AND lower(device_id) = lower($1::text) \
             ORDER BY date_last_activity DESC",
            [device_id.to_uppercase().into()],
        ))
        .await
        .expect("device session EXPLAIN must succeed")
        .expect("device session EXPLAIN must return a row");
    let plan = Value::try_get(&row, "", "QUERY PLAN").expect("EXPLAIN JSON plan must decode");
    let serialized = plan.to_string();
    assert!(
        serialized.contains("devices_lower_device_activity_idx"),
        "case-insensitive active session lookup must use its expression index: {serialized}"
    );
    transaction.rollback().await.expect("EXPLAIN rollback");
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
             WHERE schemaname = 'jellyfin' \
               AND tablename IN ('api_keys', 'devices', 'device_options', 'session_command_outbox')"
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
    assert_unique_index(&indexes, "device_options", "device_options_device_id_key");
    assert!(indexes.iter().any(|(table, name, definition)| {
        table == "device_options"
            && name == "device_options_device_id_key"
            && definition.contains("INCLUDE (custom_name)")
    }));
    assert!(indexes.iter().any(|(table, name, definition)| {
        table == "session_command_outbox"
            && name == "session_command_target_created_idx"
            && definition.contains("target_session_id")
            && definition.contains("date_created")
            && definition.contains("INCLUDE (message_type)")
    }));
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
    assert!(indexes.iter().any(|(table, name, definition)| {
        table == "devices"
            && name == "devices_lower_device_activity_idx"
            && definition.contains("lower((device_id)::text)")
            && definition.contains("date_last_activity DESC")
            && definition.contains("WHERE is_active")
    }));
}

fn assert_unique_index(indexes: &[(String, String, String)], table: &str, name: &str) {
    assert!(indexes.iter().any(|(found_table, found_name, definition)| {
        found_table == table && found_name == name && definition.contains("CREATE UNIQUE INDEX")
    }));
}
