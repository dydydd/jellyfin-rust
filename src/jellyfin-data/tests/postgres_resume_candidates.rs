use std::collections::HashSet;

use chrono::{TimeZone, Utc};
use jellyfin_data::{
    BaseItemQuery, BaseItemRepository, DatabaseConfig, NewBaseItem, NewUserData, UserDataRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_resume_candidates_";

#[tokio::test]
async fn resume_candidates_scale_from_user_data_and_preserve_rollups() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary resume database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_resume_candidates(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary resume database cleanup must succeed");
    administrator.close().await.expect("admin pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("resume candidate test task was cancelled: {error}");
    }
}

#[allow(clippy::too_many_lines)]
async fn exercise_resume_candidates(database_name: &str) {
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

    let repository = BaseItemRepository::new(database.clone());
    let user_data = UserDataRepository::new(database.clone());
    let user_id = insert_user(&database, "resume-user").await;
    let empty_user_id = insert_user(&database, "empty-resume-user").await;
    seed_unrelated_items(&database).await;

    let empty = repository
        .query_resumable(empty_user_id, &BaseItemQuery::default())
        .await
        .expect("zero-progress resume query");
    assert_eq!(empty.total_record_count, 0);
    assert!(empty.items.is_empty());

    let root = create_item(&repository, "Folder", None, true, None).await;
    let partial_series = create_item(&repository, "Series", Some(root.id), true, None).await;
    let partial_season =
        create_item(&repository, "Season", Some(partial_series.id), true, None).await;
    let played_episode =
        create_item(&repository, "Episode", Some(partial_season.id), false, None).await;
    let unplayed_episode =
        create_item(&repository, "Episode", Some(partial_season.id), false, None).await;
    insert_user_data(&user_data, user_id, played_episode.id, true, 0, 1).await;

    let progress_series = create_item(&repository, "Series", Some(root.id), true, None).await;
    let progress_season =
        create_item(&repository, "Season", Some(progress_series.id), true, None).await;
    let progress_episode = create_item(
        &repository,
        "Episode",
        Some(progress_season.id),
        false,
        None,
    )
    .await;
    insert_user_data(&user_data, user_id, progress_episode.id, false, 1_000, 2).await;

    let linked_series = create_item(&repository, "Series", Some(root.id), true, None).await;
    let linked_folder = create_item(&repository, "Folder", Some(root.id), true, None).await;
    let linked_episode =
        create_item(&repository, "Episode", Some(linked_folder.id), false, None).await;
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.linked_children (parent_id, child_id, child_type) \
             VALUES ($1, $2, 0)",
            [linked_series.id.into(), linked_folder.id.into()],
        ))
        .await
        .expect("linked folder insertion");
    insert_user_data(&user_data, user_id, linked_episode.id, false, 2_000, 3).await;

    let primary = create_item(&repository, "Movie", Some(root.id), false, None).await;
    let (low_id, high_id) = ordered_ids();
    let low_version = create_item_with_id(
        &repository,
        low_id,
        "Movie",
        Some(root.id),
        false,
        Some(primary.id),
    )
    .await;
    let high_version = create_item_with_id(
        &repository,
        high_id,
        "Movie",
        Some(root.id),
        false,
        Some(primary.id),
    )
    .await;
    insert_user_data(&user_data, user_id, low_version.id, false, 3_000, 4).await;
    insert_user_data(&user_data, user_id, high_version.id, false, 4_000, 4).await;

    let expected = HashSet::from([
        partial_series.id,
        partial_season.id,
        progress_series.id,
        progress_season.id,
        progress_episode.id,
        linked_series.id,
        linked_episode.id,
        low_version.id,
    ]);
    let query_ids = expected
        .iter()
        .copied()
        .chain([unplayed_episode.id, primary.id, high_version.id])
        .collect::<Vec<_>>();
    let page = repository
        .query_resumable(
            user_id,
            &BaseItemQuery {
                ids: query_ids.clone(),
                ..Default::default()
            },
        )
        .await
        .expect("set-based resume query");
    assert_eq!(page.total_record_count, expected.len() as u64);
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>(),
        expected
    );

    let past_end = repository
        .query_resumable(
            user_id,
            &BaseItemQuery {
                ids: query_ids,
                start_index: 100,
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .expect("resume page beyond end");
    assert_eq!(past_end.total_record_count, expected.len() as u64);
    assert!(past_end.items.is_empty());

    database.close().await.expect("database pool cleanup");
}

async fn insert_user(database: &DatabaseConnection, prefix: &str) -> Uuid {
    let user_id = Uuid::new_v4();
    let username = format!("{prefix}-{}", user_id.simple());
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.users (id, username, normalized_username) VALUES ($1, $2, $3)",
            [
                user_id.into(),
                username.clone().into(),
                username.to_uppercase().into(),
            ],
        ))
        .await
        .expect("resume user insertion");
    user_id
}

async fn seed_unrelated_items(database: &DatabaseConnection) {
    let suffix = Uuid::new_v4().simple().to_string();
    database
        .execute(Statement::from_sql_and_values(
            database.get_database_backend(),
            "INSERT INTO jellyfin.base_items (id, item_type, name, sort_name) \
             SELECT md5($1 || value::text)::uuid, 'Movie', 'Noise', 'Noise' \
             FROM generate_series(1, 10000) AS value",
            [suffix.into()],
        ))
        .await
        .expect("unrelated item insertion");
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    create_item_with_id(
        repository,
        Uuid::new_v4(),
        item_type,
        parent_id,
        is_folder,
        primary_version_id,
    )
    .await
}

async fn create_item_with_id(
    repository: &BaseItemRepository,
    id: Uuid,
    item_type: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, item_type);
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item.primary_version_id = primary_version_id;
    item.name = Some(format!("{item_type}-{id}"));
    item.sort_name = item.name.clone();
    repository.create(item).await.expect("resume item fixture")
}

async fn insert_user_data(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    played: bool,
    playback_position_ticks: i64,
    day: u32,
) {
    let mut data = NewUserData::new(item_id, user_id, format!("progress-{item_id}"));
    data.played = played;
    data.playback_position_ticks = playback_position_ticks;
    data.last_played_date = Some(Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap());
    repository.upsert(data).await.expect("resume user data");
}

fn ordered_ids() -> (Uuid, Uuid) {
    let tail = Uuid::new_v4().as_u128() & 0x0000_0000_ffff_ffff_ffff_ffff_ffff_ffff;
    (
        Uuid::from_u128(0x1111_1111_0000_0000_0000_0000_0000_0000 | tail),
        Uuid::from_u128(0xeeee_eeee_0000_0000_0000_0000_0000_0000 | tail),
    )
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
