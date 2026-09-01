use jellyfin_data::{
    BaseItemOrder, BaseItemQuery, BaseItemRepository, DatabaseConfig, ItemValueRepository,
    MediaStreamRepository, NewBaseItem, PersistedMediaStream, PersistedMediaStreamType,
    entities::{base_item, item_value::ItemValueType, user, user_data},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set,
};
use serde_json::json;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_item_order_";

#[tokio::test]
async fn postgres_extended_item_orders_use_aggregates_and_stable_ties() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary order-test database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_item_orders(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary order-test database cleanup");
    administrator.close().await.expect("admin pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("order-test task was cancelled: {error}");
    }
}

#[allow(clippy::too_many_lines)]
async fn exercise_item_orders(database_name: &str) {
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
    let values = ItemValueRepository::new(database.clone());
    let streams = MediaStreamRepository::new(database.clone());
    let user_id = create_user(&database).await;

    let mut first = new_item("Episode", "First");
    first.parent_index_number = Some(1);
    first.index_number = Some(1);
    first.data = Some(json!({
        "Album": "A Album",
        "SeriesName": "A Series",
        "StartDate": "2026-01-02T00:00:00Z",
        "DateLastMediaAdded": "2026-02-02T00:00:00Z"
    }));
    let first = repository.create(first).await.expect("first item");
    let mut second = new_item("Episode", "Second");
    second.parent_index_number = Some(1);
    second.index_number = Some(2);
    second.data = Some(json!({
        "Album": "B Album",
        "SeriesName": "B Series",
        "StartDate": "2026-01-01T00:00:00Z",
        "DateLastMediaAdded": "2026-02-01T00:00:00Z"
    }));
    let second = repository.create(second).await.expect("second item");
    let mut special = new_item("Episode", "Special");
    special.parent_index_number = Some(0);
    special.index_number = Some(1);
    special.data = Some(json!({
        "AirsBeforeSeasonNumber": 1,
        "AirsBeforeEpisodeNumber": 1
    }));
    let special = repository.create(special).await.expect("special item");
    let mut movie = new_item("Movie", "Movie");
    movie.official_rating = Some("PG-13".to_owned());
    let movie = repository.create(movie).await.expect("movie item");
    let mut folder = new_item("Folder", "Folder");
    folder.is_folder = true;
    let folder = repository.create(folder).await.expect("folder item");

    values
        .link(second.id, ItemValueType::Artist, "Zed")
        .await
        .expect("second artist");
    values
        .link(first.id, ItemValueType::Artist, "Ann")
        .await
        .expect("first artist");
    values
        .link(second.id, ItemValueType::AlbumArtist, "Zed Album Artist")
        .await
        .expect("second album artist");
    values
        .link(first.id, ItemValueType::AlbumArtist, "Ann Album Artist")
        .await
        .expect("first album artist");
    values
        .link(second.id, ItemValueType::Studios, "Zed Studio")
        .await
        .expect("second studio");
    values
        .link(first.id, ItemValueType::Studios, "Ann Studio")
        .await
        .expect("first studio");
    streams
        .replace(second.id, &[video_stream(0, 9_000), video_stream(1, 7_000)])
        .await
        .expect("second streams");
    streams
        .replace(first.id, &[video_stream(0, 4_000)])
        .await
        .expect("first stream");
    user_data::ActiveModel {
        user_id: Set(user_id),
        item_id: Set(second.id),
        custom_data_key: Set("default".to_owned()),
        played: Set(true),
        is_favorite: Set(true),
        ..Default::default()
    }
    .insert(&database)
    .await
    .expect("second user data");

    let fixture_ids = [first.id, second.id, special.id, movie.id, folder.id];
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::AiredEpisodeOrderAscending,
        &[first.id, second.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::AiredEpisodeOrderDescending,
        &[special.id, second.id, first.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::AlbumAscending,
        &[first.id, second.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::AlbumArtistAscending,
        &[first.id, second.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::ArtistAscending,
        &[first.id, second.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::OfficialRatingDescending,
        &[movie.id, first.id, folder.id, second.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::StartDateDescending,
        &[first.id, second.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::IsFolderDescending,
        &[folder.id, first.id, movie.id, second.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::IsUnplayedAscending,
        &[second.id, first.id, folder.id, movie.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::IsPlayedDescending,
        &[second.id, first.id, folder.id, movie.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::SeriesSortNameDescending,
        &[second.id, first.id, folder.id, movie.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::VideoBitRateDescending,
        &[second.id, first.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::StudioAscending,
        &[first.id, second.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::IsFavoriteOrLikedDescending,
        &[second.id, first.id, folder.id, movie.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::DateLastContentAddedDescending,
        &[first.id, second.id, folder.id, movie.id, special.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::ParentIndexNumberAscending,
        &[special.id, first.id, second.id, folder.id, movie.id],
    )
    .await;
    assert_order(
        &repository,
        user_id,
        BaseItemOrder::IndexNumberDescending,
        &[second.id, first.id, special.id, folder.id, movie.id],
    )
    .await;

    base_item::Entity::delete_many()
        .filter(base_item::Column::Id.is_in(fixture_ids))
        .exec(&database)
        .await
        .expect("order fixture cleanup");
    database.close().await.expect("order pool cleanup");
}

fn new_item(item_type: &'static str, name: &'static str) -> NewBaseItem {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item
}

fn video_stream(index: i32, bit_rate: i32) -> PersistedMediaStream {
    PersistedMediaStream {
        stream_index: index,
        stream_type: PersistedMediaStreamType::Video,
        bit_rate: Some(bit_rate),
        codec: None,
        language: None,
        channel_layout: None,
        profile: None,
        aspect_ratio: None,
        path: None,
        is_interlaced: None,
        channels: None,
        sample_rate: None,
        is_default: false,
        is_forced: false,
        is_external: false,
        is_original: false,
        height: None,
        width: None,
        average_frame_rate: None,
        real_frame_rate: None,
        level: None,
        pixel_format: None,
        bit_depth: None,
        is_anamorphic: None,
        ref_frames: None,
        codec_tag: None,
        comment: None,
        nal_length_size: None,
        is_avc: None,
        title: None,
        time_base: None,
        codec_time_base: None,
        color_range: None,
        color_primaries: None,
        color_space: None,
        color_transfer: None,
        dv_version_major: None,
        dv_version_minor: None,
        dv_profile: None,
        dv_level: None,
        rpu_present_flag: None,
        el_present_flag: None,
        bl_present_flag: None,
        dv_bl_signal_compatibility_id: None,
        is_hearing_impaired: None,
        rotation: None,
        hdr10_plus_present_flag: None,
    }
}

async fn create_user(database: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    user::ActiveModel {
        id: Set(id),
        username: Set(format!("order-user-{}", id.simple())),
        normalized_username: Set(format!("order-user-{}", id.simple())),
        ..Default::default()
    }
    .insert(database)
    .await
    .expect("order test user");
    id
}

async fn assert_order(
    repository: &BaseItemRepository,
    user_id: Uuid,
    order: BaseItemOrder,
    expected: &[Uuid],
) {
    let page = repository
        .query(&BaseItemQuery {
            user_id: Some(user_id),
            recursive: true,
            order,
            limit: Some(10),
            enable_total_record_count: Some(false),
            ..Default::default()
        })
        .await
        .expect("ordered item query");
    let actual: Vec<_> = page.items.iter().map(|item| item.id).collect();
    assert!(
        actual.starts_with(expected),
        "order {order:?}: expected prefix {expected:?}, actual {actual:?}"
    );
}
