use chrono::{Duration, TimeZone, Utc};
use jellyfin_data::{
    DatabaseConfig, NewUserData, UserDataPatch, UserDataQuery, UserDataRepository,
    entities::{user, user_data},
};
use jellyfin_migration::CreateUserDataMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Statement,
    TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

#[tokio::test]
async fn postgres_user_data_vertical_slice() {
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = Uuid::new_v4();
    let other_user_id = Uuid::new_v4();
    insert_user(&database, user_id, &format!("UserData-{suffix}")).await;
    insert_user(&database, other_user_id, &format!("OtherUserData-{suffix}")).await;

    let repository = UserDataRepository::new(database.clone());
    assert_crud_and_key_resolution(&repository, user_id, other_user_id).await;
    assert_query_filters(&repository, user_id).await;
    assert_concurrent_upsert(&repository, user_id).await;
    assert_cascade_cleanup(&database, user_id, other_user_id).await;
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    CreateUserDataMigration
        .up(&schema)
        .await
        .expect("reapplying user data DDL must succeed");
    CreateUserDataMigration
        .up(&schema)
        .await
        .expect("user data DDL must remain idempotent");
    assert_postgres_schema(&database).await;
    database
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
        .expect("user data test user must be inserted");
}

async fn assert_crud_and_key_resolution(
    repository: &UserDataRepository,
    user_id: Uuid,
    other_user_id: Uuid,
) {
    let item_id = Uuid::new_v4();
    let retired_key = "Author-Old Album-0001Old File Name";
    let current_key = "Author-Series-0001Book Title";
    let id_key = item_id.simple().to_string();

    let mut retired = NewUserData::new(item_id, user_id, retired_key);
    retired.playback_position_ticks = 111;
    repository
        .upsert(retired)
        .await
        .expect("retired key row must be inserted");

    let last_played = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
    let retention = last_played + Duration::days(30);
    let mut current = NewUserData::new(item_id, user_id, current_key);
    current.rating = Some(8.5);
    current.playback_position_ticks = 222;
    current.play_count = 3;
    current.is_favorite = true;
    current.last_played_date = Some(last_played);
    current.played = true;
    current.audio_stream_index = Some(2);
    current.subtitle_stream_index = Some(4);
    current.likes = Some(true);
    current.retention_date = Some(retention);
    let inserted = repository
        .upsert(current)
        .await
        .expect("current key row must be inserted");
    assert_eq!(inserted.rating, Some(8.5));
    assert_eq!(inserted.play_count, 3);
    assert_eq!(inserted.audio_stream_index, Some(2));
    assert_eq!(inserted.subtitle_stream_index, Some(4));
    assert_eq!(inserted.likes, Some(true));
    assert_eq!(inserted.retention_date, Some(retention));

    let all_keys = repository
        .get_for_item(item_id, user_id)
        .await
        .expect("all key variants must load");
    assert_eq!(all_keys.len(), 2);
    let resolved = repository
        .resolve_by_keys(item_id, user_id, &[current_key.to_owned(), id_key.clone()])
        .await
        .expect("key-priority lookup must succeed")
        .expect("current key row must resolve");
    assert_eq!(resolved.custom_data_key, current_key);
    assert_eq!(resolved.playback_position_ticks, 222);

    assert_patch(repository, item_id, user_id, current_key).await;

    let second_item = Uuid::new_v4();
    let second_id_key = second_item.simple().to_string();
    let mut second_retired = NewUserData::new(second_item, user_id, retired_key);
    second_retired.playback_position_ticks = 555;
    repository
        .upsert(second_retired)
        .await
        .expect("second retired key row must be inserted");
    let mut second_id = NewUserData::new(second_item, user_id, &second_id_key);
    second_id.playback_position_ticks = 666;
    repository
        .upsert(second_id)
        .await
        .expect("secondary current key row must be inserted");
    let resolved = repository
        .resolve_by_keys(
            second_item,
            user_id,
            &["missing-primary".to_owned(), second_id_key],
        )
        .await
        .expect("secondary key lookup must succeed")
        .expect("secondary current key row must resolve");
    assert_eq!(resolved.playback_position_ticks, 666);

    let retired_only_item = Uuid::new_v4();
    let mut retired_only = NewUserData::new(retired_only_item, user_id, retired_key);
    retired_only.playback_position_ticks = 777;
    repository
        .upsert(retired_only)
        .await
        .expect("retired-only row must be inserted");
    let resolved = repository
        .resolve_by_keys(retired_only_item, user_id, &["new-key".to_owned()])
        .await
        .expect("retired fallback lookup must succeed")
        .expect("retired row must be returned");
    assert_eq!(resolved.playback_position_ticks, 777);

    let mut other_user = NewUserData::new(item_id, other_user_id, current_key);
    other_user.playback_position_ticks = 999;
    repository
        .upsert(other_user)
        .await
        .expect("other user's row must be inserted");
    let own = repository
        .resolve_by_keys(item_id, user_id, &[current_key.to_owned()])
        .await
        .expect("user-scoped lookup must succeed")
        .expect("own row must resolve");
    assert_eq!(own.playback_position_ticks, 444);
}

async fn assert_patch(
    repository: &UserDataRepository,
    item_id: Uuid,
    user_id: Uuid,
    current_key: &str,
) {
    let patched = repository
        .patch(
            item_id,
            user_id,
            current_key,
            UserDataPatch {
                rating: Some(None),
                playback_position_ticks: Some(444),
                play_count: Some(5),
                is_favorite: Some(false),
                last_played_date: Some(None),
                played: Some(false),
                audio_stream_index: Some(None),
                subtitle_stream_index: Some(Some(6)),
                likes: Some(Some(false)),
                retention_date: Some(None),
            },
        )
        .await
        .expect("user data patch must succeed")
        .expect("patched user data must exist");
    assert_eq!(patched.rating, None);
    assert_eq!(patched.playback_position_ticks, 444);
    assert_eq!(patched.play_count, 5);
    assert!(!patched.is_favorite);
    assert_eq!(patched.last_played_date, None);
    assert!(!patched.played);
    assert_eq!(patched.audio_stream_index, None);
    assert_eq!(patched.subtitle_stream_index, Some(6));
    assert_eq!(patched.likes, Some(false));
    assert_eq!(patched.retention_date, None);
}

async fn assert_query_filters(repository: &UserDataRepository, user_id: Uuid) {
    let base_date = Utc.with_ymd_and_hms(2026, 7, 21, 8, 0, 0).unwrap();
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let third_id = Uuid::new_v4();

    let mut first = NewUserData::new(first_id, user_id, "query-first");
    first.played = true;
    first.is_favorite = true;
    first.playback_position_ticks = 100;
    first.last_played_date = Some(base_date);
    repository.upsert(first).await.expect("first query row");

    let mut second = NewUserData::new(second_id, user_id, "query-second");
    second.last_played_date = Some(base_date + Duration::hours(1));
    repository.upsert(second).await.expect("second query row");

    let mut third = NewUserData::new(third_id, user_id, "query-third");
    third.played = true;
    third.playback_position_ticks = 200;
    third.last_played_date = Some(base_date + Duration::hours(2));
    repository.upsert(third).await.expect("third query row");

    let item_ids = vec![first_id, second_id, third_id];
    let played = repository
        .query(&UserDataQuery {
            user_id,
            item_ids: item_ids.clone(),
            played: Some(true),
            ..Default::default()
        })
        .await
        .expect("played query must succeed");
    assert_eq!(played.len(), 2);

    let favorite = repository
        .query(&UserDataQuery {
            user_id,
            item_ids: item_ids.clone(),
            is_favorite: Some(true),
            ..Default::default()
        })
        .await
        .expect("favorite query must succeed");
    assert_eq!(favorite.len(), 1);
    assert_eq!(favorite[0].item_id, first_id);

    let resume = repository
        .query(&UserDataQuery {
            user_id,
            item_ids: item_ids.clone(),
            has_playback_position: Some(true),
            ..Default::default()
        })
        .await
        .expect("resume query must succeed");
    assert_eq!(resume.len(), 2);

    let recent = repository
        .query(&UserDataQuery {
            user_id,
            item_ids,
            min_last_played_date: Some(base_date + Duration::minutes(30)),
            max_last_played_date: Some(base_date + Duration::hours(3)),
            order_by_last_played_desc: true,
            limit: Some(1),
            ..Default::default()
        })
        .await
        .expect("recent query must succeed");
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].item_id, third_id);
}

async fn assert_concurrent_upsert(repository: &UserDataRepository, user_id: Uuid) {
    let item_id = Uuid::new_v4();
    let mut first = NewUserData::new(item_id, user_id, "concurrent");
    first.play_count = 1;
    let mut second = first.clone();
    second.play_count = 2;
    let first_repository = repository.clone();
    let second_repository = repository.clone();

    let (first_result, second_result) = tokio::join!(
        first_repository.upsert(first),
        second_repository.upsert(second)
    );
    first_result.expect("first concurrent upsert must succeed");
    second_result.expect("second concurrent upsert must succeed");

    let rows = repository
        .get_for_item(item_id, user_id)
        .await
        .expect("concurrently upserted row must load");
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].play_count, 1 | 2));
}

async fn assert_cascade_cleanup(database: &DatabaseConnection, user_id: Uuid, other_user_id: Uuid) {
    user::Entity::delete_by_id(user_id)
        .exec(database)
        .await
        .expect("test user deletion must succeed");
    let remaining = user_data::Entity::find()
        .filter(user_data::Column::UserId.eq(user_id))
        .all(database)
        .await
        .expect("cascade verification must succeed");
    assert!(remaining.is_empty());
    user::Entity::delete_by_id(other_user_id)
        .exec(database)
        .await
        .expect("other test user deletion must succeed");
}

async fn assert_postgres_schema(database: &DatabaseConnection) {
    let primary_key = database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT string_agg(attribute.attname, ',' ORDER BY key.ordinality) AS columns \
             FROM pg_constraint AS catalog_constraint \
             CROSS JOIN LATERAL unnest(catalog_constraint.conkey) WITH ORDINALITY AS key(attnum, ordinality) \
             JOIN pg_attribute AS attribute ON attribute.attrelid = catalog_constraint.conrelid \
                 AND attribute.attnum = key.attnum \
             WHERE catalog_constraint.conrelid = 'jellyfin.user_data'::regclass \
                 AND catalog_constraint.contype = 'p'"
                .to_owned(),
        ))
        .await
        .expect("primary key catalog query must succeed")
        .expect("user data primary key must exist");
    let columns =
        String::try_get(&primary_key, "", "columns").expect("primary key columns must be text");
    assert_eq!(columns, "item_id,user_id,custom_data_key");

    let rows = database
        .query_all(Statement::from_string(
            database.get_database_backend(),
            "SELECT indexname, indexdef FROM pg_indexes \
             WHERE schemaname = 'jellyfin' AND tablename = 'user_data'"
                .to_owned(),
        ))
        .await
        .expect("user data index catalog query must succeed");
    let indexes: Vec<(String, String)> = rows
        .into_iter()
        .map(|row| {
            (
                String::try_get(&row, "", "indexname").expect("index name must be text"),
                String::try_get(&row, "", "indexdef").expect("index definition must be text"),
            )
        })
        .collect();

    assert_partial_index(&indexes, "user_data_played_idx", "where played");
    assert_partial_index(&indexes, "user_data_favorite_idx", "where is_favorite");
    assert_partial_index(
        &indexes,
        "user_data_resume_idx",
        "playback_position_ticks > 0",
    );
    assert_partial_index(
        &indexes,
        "user_data_last_played_idx",
        "last_played_date is not null",
    );
}

fn assert_partial_index(indexes: &[(String, String)], name: &str, predicate: &str) {
    assert!(indexes.iter().any(|(found_name, definition)| {
        found_name == name
            && definition
                .to_ascii_lowercase()
                .replace(['(', ')'], "")
                .contains(predicate)
    }));
}
