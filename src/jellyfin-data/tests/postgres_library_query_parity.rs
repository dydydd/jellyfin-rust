use jellyfin_data::{
    BaseItemQuery, BaseItemRepository, CollectionRepository, DatabaseConfig, ItemValueRepository,
    NewBaseItem, NewUserData, UserDataRepository,
    entities::{item_value::ItemValueType, user},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Statement};
use serde_json::{Value, json};
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static TEST_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn default_query_excludes_owned_non_extra_items_and_matches_original_title() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let repository = BaseItemRepository::new(database.clone());
    let root = repository.ensure_user_root().await.expect("user root");
    let container = create_item(
        &repository,
        "Folder",
        &format!("Container {suffix}"),
        Some(root.id),
        true,
        None,
        None,
    )
    .await;

    let primary = create_item(
        &repository,
        "Movie",
        &format!("Primary {suffix}"),
        Some(container.id),
        false,
        None,
        None,
    )
    .await;
    let alternate = create_item(
        &repository,
        "Movie",
        &format!("Alternate {suffix}"),
        Some(container.id),
        false,
        Some(primary.id),
        None,
    )
    .await;
    let owned_non_extra = create_item(
        &repository,
        "Movie",
        &format!("Owned Non-Extra {suffix}"),
        Some(container.id),
        false,
        None,
        Some(json!({ "OwnerId": primary.id })),
    )
    .await;
    let extra = create_item(
        &repository,
        "Movie",
        &format!("Trailer {suffix}"),
        Some(container.id),
        false,
        None,
        Some(json!({ "OwnerId": primary.id, "ExtraType": "Trailer" })),
    )
    .await;

    let page = repository
        .query(&BaseItemQuery {
            ids: vec![primary.id, alternate.id, owned_non_extra.id, extra.id],
            ..Default::default()
        })
        .await
        .expect("default library query");
    let returned = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
    assert_eq!(returned.len(), 2);
    assert!(returned.contains(&primary.id));
    assert!(returned.contains(&extra.id));

    let original_title = create_item(
        &repository,
        "Movie",
        &format!("Localized {suffix}"),
        Some(container.id),
        false,
        None,
        Some(json!({ "OriginalTitle": "Matrix 1999" })),
    )
    .await;
    let search = repository
        .query(&BaseItemQuery {
            ids: vec![primary.id, alternate.id, original_title.id],
            search_term: Some("Matrix".to_owned()),
            ..Default::default()
        })
        .await
        .expect("searchTerm with OriginalTitle");
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].id, original_title.id);

    cleanup(&repository, container.id).await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn played_filter_rolls_up_series_seasons_and_box_sets() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = insert_user(&database, &format!("Played-{suffix}")).await;
    let repository = BaseItemRepository::new(database.clone());
    let user_data = UserDataRepository::new(database.clone());
    let root = repository.ensure_user_root().await.expect("user root");
    let container = create_item(
        &repository,
        "Folder",
        &format!("Container {suffix}"),
        Some(root.id),
        true,
        None,
        None,
    )
    .await;

    let series = create_item(
        &repository,
        "Series",
        &format!("Series {suffix}"),
        Some(container.id),
        true,
        None,
        None,
    )
    .await;
    let season = create_item(
        &repository,
        "Season",
        &format!("Season {suffix}"),
        Some(series.id),
        true,
        None,
        None,
    )
    .await;
    let played_episode = create_item(
        &repository,
        "Episode",
        &format!("Played Episode {suffix}"),
        Some(season.id),
        false,
        None,
        None,
    )
    .await;
    insert_user_data(&user_data, user_id, played_episode.id, true, 0).await;

    let played_query = BaseItemQuery {
        ids: vec![series.id, season.id],
        user_id: Some(user_id),
        is_played: Some(true),
        ..Default::default()
    };
    let page = repository
        .query(&played_query)
        .await
        .expect("played folder query");
    assert_eq!(page.total_record_count, 2);
    assert!(
        page.items
            .iter()
            .all(|item| item.id == series.id || item.id == season.id)
    );

    let _unplayed_episode = create_item(
        &repository,
        "Episode",
        &format!("Unplayed Episode {suffix}"),
        Some(season.id),
        false,
        None,
        None,
    )
    .await;
    let played_page = repository
        .query(&played_query)
        .await
        .expect("partially played folder query");
    assert_eq!(played_page.total_record_count, 0);
    let unplayed_page = repository
        .query(&BaseItemQuery {
            is_played: Some(false),
            ..played_query.clone()
        })
        .await
        .expect("partially played unplayed folder query");
    assert_eq!(unplayed_page.total_record_count, 2);

    let box_set_first = create_item(
        &repository,
        "Movie",
        &format!("Box Set First {suffix}"),
        Some(container.id),
        false,
        None,
        None,
    )
    .await;
    let box_set_second = create_item(
        &repository,
        "Movie",
        &format!("Box Set Second {suffix}"),
        Some(container.id),
        false,
        None,
        None,
    )
    .await;
    insert_user_data(&user_data, user_id, box_set_first.id, true, 0).await;
    let box_set = CollectionRepository::new(database.clone())
        .create(
            Uuid::new_v4(),
            Some(format!("Box Set {suffix}")),
            Some(container.id),
            false,
            &[box_set_first.id, box_set_second.id],
        )
        .await
        .expect("box set creation");

    let box_played = repository
        .query(&BaseItemQuery {
            ids: vec![box_set.id],
            user_id: Some(user_id),
            is_played: Some(true),
            ..Default::default()
        })
        .await
        .expect("box set played query");
    assert_eq!(box_played.total_record_count, 0);
    let box_unplayed = repository
        .query(&BaseItemQuery {
            ids: vec![box_set.id],
            user_id: Some(user_id),
            is_played: Some(false),
            ..Default::default()
        })
        .await
        .expect("box set unplayed query");
    assert_eq!(box_unplayed.total_record_count, 1);

    insert_user_data(&user_data, user_id, box_set_second.id, true, 0).await;
    let box_played = repository
        .query(&BaseItemQuery {
            ids: vec![box_set.id],
            user_id: Some(user_id),
            is_played: Some(true),
            ..Default::default()
        })
        .await
        .expect("completed box set played query");
    assert_eq!(box_played.total_record_count, 1);

    cleanup(&repository, container.id).await;
    user::Entity::delete_by_id(user_id)
        .exec(&database)
        .await
        .expect("played user cleanup");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn resumable_filter_rolls_up_partially_watched_series_and_seasons() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let user_id = insert_user(&database, &format!("Resume-{suffix}")).await;
    let repository = BaseItemRepository::new(database.clone());
    let user_data = UserDataRepository::new(database.clone());
    let root = repository.ensure_user_root().await.expect("user root");
    let container = create_item(
        &repository,
        "Folder",
        &format!("Container {suffix}"),
        Some(root.id),
        true,
        None,
        None,
    )
    .await;

    let series = create_item(
        &repository,
        "Series",
        &format!("Series {suffix}"),
        Some(container.id),
        true,
        None,
        None,
    )
    .await;
    let season = create_item(
        &repository,
        "Season",
        &format!("Season {suffix}"),
        Some(series.id),
        true,
        None,
        None,
    )
    .await;
    let watched_episode = create_item(
        &repository,
        "Episode",
        &format!("Watched Episode {suffix}"),
        Some(season.id),
        false,
        None,
        None,
    )
    .await;
    let unwatched_episode = create_item(
        &repository,
        "Episode",
        &format!("Unwatched Episode {suffix}"),
        Some(season.id),
        false,
        None,
        None,
    )
    .await;
    insert_user_data(&user_data, user_id, watched_episode.id, true, 0).await;

    let folder_ids = vec![series.id, season.id];
    let resumable = repository
        .query_resumable(
            user_id,
            &BaseItemQuery {
                ids: folder_ids.clone(),
                ..Default::default()
            },
        )
        .await
        .expect("folder resume query");
    assert_eq!(resumable.total_record_count, 2);
    assert!(
        resumable
            .items
            .iter()
            .all(|item| item.id == series.id || item.id == season.id)
    );

    let not_resumable = repository
        .query(&BaseItemQuery {
            ids: folder_ids,
            user_id: Some(user_id),
            is_resumable: Some(false),
            ..Default::default()
        })
        .await
        .expect("folder not-resumable query");
    assert_eq!(not_resumable.total_record_count, 0);

    insert_user_data(&user_data, user_id, unwatched_episode.id, false, 1_000).await;
    let in_progress_folders = repository
        .query_resumable(
            user_id,
            &BaseItemQuery {
                ids: vec![series.id, season.id],
                ..Default::default()
            },
        )
        .await
        .expect("in-progress descendant resume query");
    assert_eq!(in_progress_folders.total_record_count, 2);

    cleanup(&repository, container.id).await;
    user::Entity::delete_by_id(user_id)
        .exec(&database)
        .await
        .expect("resume user cleanup");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn parental_control_filters_inherit_ratings_tags_and_unrated_type() {
    let _guard = TEST_LOCK.lock().await;
    let database = prepare_database().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let repository = BaseItemRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let root = repository.ensure_user_root().await.expect("user root");
    let container = create_item(
        &repository,
        "Folder",
        &format!("Parental Container {suffix}"),
        Some(root.id),
        true,
        None,
        None,
    )
    .await;

    let mut series = create_item(
        &repository,
        "Series",
        &format!("Parental Series {suffix}"),
        Some(container.id),
        true,
        None,
        None,
    )
    .await;
    series.official_rating = Some("PG-13".to_owned());
    let series = repository.update(series).await.expect("series rating");
    values
        .link(series.id, ItemValueType::Tags, "horror")
        .await
        .expect("series tag");

    let season = create_item(
        &repository,
        "Season",
        &format!("Parental Season {suffix}"),
        Some(series.id),
        true,
        None,
        None,
    )
    .await;
    let mut episode = create_item(
        &repository,
        "Episode",
        &format!("Parental Episode {suffix}"),
        Some(season.id),
        false,
        None,
        None,
    )
    .await;
    episode.series_id = Some(series.id);
    episode.top_parent_id = Some(container.id);
    let episode = repository.update(episode).await.expect("episode hierarchy");

    let blocked = repository
        .query(&BaseItemQuery {
            ids: vec![episode.id, season.id],
            blocked_tags: vec!["horror".to_owned()],
            ..Default::default()
        })
        .await
        .expect("blocked inherited tag query");
    assert_eq!(blocked.total_record_count, 0);

    let allowed = repository
        .query(&BaseItemQuery {
            ids: vec![episode.id, season.id],
            allowed_tags: vec!["horror".to_owned()],
            ..Default::default()
        })
        .await
        .expect("allowed inherited tag query");
    assert_eq!(allowed.total_record_count, 2);

    let untagged_movie = create_item(
        &repository,
        "Movie",
        &format!("Parental Untagged Movie {suffix}"),
        Some(container.id),
        false,
        None,
        None,
    )
    .await;
    let allowed_movie = repository
        .query(&BaseItemQuery {
            ids: vec![untagged_movie.id],
            allowed_tags: vec!["horror".to_owned()],
            ..Default::default()
        })
        .await
        .expect("allowed tag movie query");
    assert_eq!(allowed_movie.total_record_count, 0);

    let rating_blocked = repository
        .query(&BaseItemQuery {
            ids: vec![episode.id],
            allowed_parental_ratings: vec!["10".to_owned(), "10+".to_owned()],
            ..Default::default()
        })
        .await
        .expect("inherited rating block query");
    assert_eq!(rating_blocked.total_record_count, 0);

    let rating_allowed = repository
        .query(&BaseItemQuery {
            ids: vec![episode.id],
            allowed_parental_ratings: vec!["pg-13".to_owned()],
            ..Default::default()
        })
        .await
        .expect("inherited rating allow query");
    assert_eq!(rating_allowed.total_record_count, 1);

    let mut custom_rated = create_item(
        &repository,
        "Movie",
        &format!("Parental Custom Rated Movie {suffix}"),
        Some(container.id),
        false,
        None,
        Some(json!({ "CustomRating": "FSK-18" })),
    )
    .await;
    custom_rated.official_rating = Some("PG".to_owned());
    let custom_rated = repository
        .update(custom_rated)
        .await
        .expect("custom rated movie");
    let custom_blocked = repository
        .query(&BaseItemQuery {
            ids: vec![custom_rated.id],
            allowed_parental_ratings: vec!["pg".to_owned(), "pg-13".to_owned()],
            ..Default::default()
        })
        .await
        .expect("custom rating block query");
    assert_eq!(custom_blocked.total_record_count, 0);

    let custom_series = create_item(
        &repository,
        "Series",
        &format!("Parental Custom Series {suffix}"),
        Some(container.id),
        true,
        None,
        Some(json!({ "CustomRating": "FSK-18" })),
    )
    .await;
    let custom_series = repository
        .update(custom_series)
        .await
        .expect("custom series");
    let mut custom_series_child = create_item(
        &repository,
        "Episode",
        &format!("Parental Custom Series Child {suffix}"),
        Some(custom_series.id),
        false,
        None,
        None,
    )
    .await;
    custom_series_child.official_rating = Some("PG".to_owned());
    custom_series_child.series_id = Some(custom_series.id);
    custom_series_child.top_parent_id = Some(container.id);
    let custom_series_child = repository
        .update(custom_series_child)
        .await
        .expect("custom series child");
    let inherited_custom_blocked = repository
        .query(&BaseItemQuery {
            ids: vec![custom_series_child.id],
            allowed_parental_ratings: vec!["pg".to_owned()],
            ..Default::default()
        })
        .await
        .expect("inherited custom rating block query");
    assert_eq!(inherited_custom_blocked.total_record_count, 0);

    let unrated_blocked = repository
        .query(&BaseItemQuery {
            ids: vec![untagged_movie.id],
            block_unrated_items: vec!["Movie".to_owned()],
            ..Default::default()
        })
        .await
        .expect("unrated movie block query");
    assert_eq!(unrated_blocked.total_record_count, 0);

    let mut rated_movie = create_item(
        &repository,
        "Movie",
        &format!("Parental Rated Movie {suffix}"),
        Some(container.id),
        false,
        None,
        None,
    )
    .await;
    rated_movie.official_rating = Some("PG-13".to_owned());
    let rated_movie = repository.update(rated_movie).await.expect("rated movie");
    let unrated_allowed = repository
        .query(&BaseItemQuery {
            ids: vec![rated_movie.id],
            block_unrated_items: vec!["Movie".to_owned()],
            ..Default::default()
        })
        .await
        .expect("rated movie unrated block query");
    assert_eq!(unrated_allowed.total_record_count, 1);

    let unrated_series_child = repository
        .query(&BaseItemQuery {
            ids: vec![episode.id],
            block_unrated_items: vec!["Series".to_owned()],
            ..Default::default()
        })
        .await
        .expect("unrated series child query");
    assert_eq!(unrated_series_child.total_record_count, 1);

    cleanup(&repository, container.id).await;
}

async fn prepare_database() -> DatabaseConnection {
    static DATABASE: OnceCell<DatabaseConnection> = OnceCell::const_new();
    DATABASE
        .get_or_init(|| async {
            let database = jellyfin_data::connect(&DatabaseConfig {
                max_connections: 1,
                min_connections: 1,
                ..DatabaseConfig::default()
            })
            .await
            .expect("local PostgreSQL must be available");
            jellyfin_data::migrate(&database)
                .await
                .expect("parity migrations must succeed");
            database
        })
        .await
        .clone()
}

async fn insert_user(database: &DatabaseConnection, username: &str) -> Uuid {
    let user_id = Uuid::new_v4();
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
        .expect("parity user insertion");
    user_id
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    primary_version_id: Option<Uuid>,
    data: Option<Value>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item.media_type = if is_folder {
        None
    } else {
        Some("Video".to_owned())
    };
    item.primary_version_id = primary_version_id;
    item.data = data;
    repository.create(item).await.expect("parity item creation")
}

async fn insert_user_data(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    played: bool,
    position: i64,
) {
    let mut data = NewUserData::new(item_id, user_id, item_id.simple().to_string());
    data.played = played;
    data.playback_position_ticks = position;
    repository
        .upsert(data)
        .await
        .expect("parity user data insertion");
}

async fn cleanup(repository: &BaseItemRepository, id: Uuid) {
    repository.delete(id).await.expect("parity fixture cleanup");
}
