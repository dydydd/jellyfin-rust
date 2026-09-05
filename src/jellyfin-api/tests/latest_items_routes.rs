use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewUserData,
    UserDataRepository,
    entities::{base_item, item_value},
};
use jellyfin_model::UserPolicy;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, sea_query::Expr,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Latest Items Tests\", DeviceId=\"latest-items-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_latest_items_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn latest_items_follow_official_user_library_contract() {
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
        exercise_latest_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_latest_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let fixture = Fixture::new(database.clone()).await;

    assert_auth_and_target_user_rules(&fixture).await;
    assert_latest_defaults_hide_played_and_sort_by_created(&fixture).await;
    assert_is_played_and_legacy_routes(&fixture).await;
    assert_default_grouping_and_explicit_ungrouping(&fixture).await;
    assert_tv_latest_window_grouping(&fixture).await;
    assert_album_ancestor_grouping(&fixture).await;
    assert_latest_dto_options_and_image_fields(&fixture).await;

    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    restricted_user_token: String,
    parent_id: Uuid,
    old_movie_id: Uuid,
    new_movie_id: Uuid,
    played_movie_id: Uuid,
    episode_id: Uuid,
    series_id: Uuid,
    first_series_episode_id: Uuid,
    second_series_episode_id: Uuid,
    single_series_id: Uuid,
    over_24_series_id: Uuid,
    over_24_new_episode_id: Uuid,
    multi_season_series_id: Uuid,
    multi_recent_season_id: Uuid,
    cross_season_series_id: Uuid,
    hidden_series_episode_id: Uuid,
    saturated_parent_id: Uuid,
    saturated_new_series_id: Uuid,
    saturated_old_series_id: Uuid,
    album_id: Uuid,
    album_track_id: Uuid,
    photo_album_id: Uuid,
    first_photo_id: Uuid,
    second_photo_id: Uuid,
    single_photo_album_id: Uuid,
    single_photo_id: Uuid,
    hidden_album_track_id: Uuid,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("latest-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("latest-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("latest-other-{suffix}"))
            .await
            .expect("other user creation");
        let restricted_user = users
            .create(&format!("latest-restricted-{suffix}"))
            .await
            .expect("restricted user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let restricted_user_token = session(
            &devices,
            restricted_user.id,
            &format!("restricted-{suffix}"),
        )
        .await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("root");
        let parent = create_item(&items, "Folder", "Latest Parent", root.id).await;
        let old_movie = create_item(&items, "Movie", "Old Movie", parent.id).await;
        let new_movie = create_item(&items, "Movie", "New Movie", parent.id).await;
        let played_movie = create_item(&items, "Movie", "Played Movie", parent.id).await;
        let episode = create_item(&items, "Episode", "Newest Episode", parent.id).await;
        let series = create_item(&items, "Series", "Grouped Series", parent.id).await;
        let season = create_item(&items, "Season", "Grouped Season", series.id).await;
        let first_series_episode = create_episode(
            &items,
            "First Grouped Episode",
            season.id,
            series.id,
            season.id,
        )
        .await;
        let second_series_episode = create_episode(
            &items,
            "Second Grouped Episode",
            season.id,
            series.id,
            season.id,
        )
        .await;
        let single_series = create_item(&items, "Series", "Single Episode Series", parent.id).await;
        let single_season = create_item(&items, "Season", "Single Season", single_series.id).await;
        let single_series_episode = create_episode(
            &items,
            "Only Series Episode",
            single_season.id,
            single_series.id,
            single_season.id,
        )
        .await;
        let mut single_series_alternate = NewBaseItem::new(Uuid::new_v4(), "Episode");
        single_series_alternate.name = Some("Only Series Episode Alternate".to_owned());
        single_series_alternate.sort_name = single_series_alternate.name.clone();
        single_series_alternate.parent_id = Some(single_season.id);
        single_series_alternate.media_type = Some("Video".to_owned());
        single_series_alternate.series_id = Some(single_series.id);
        single_series_alternate.season_id = Some(single_season.id);
        single_series_alternate.primary_version_id = Some(single_series_episode.id);
        let single_series_alternate = items
            .create(single_series_alternate)
            .await
            .expect("alternate episode version");

        let over_24_series = create_item(&items, "Series", "Over 24 Hour Series", parent.id).await;
        let over_24_season =
            create_item(&items, "Season", "Over 24 Hour Season", over_24_series.id).await;
        let over_24_old_episode = create_episode(
            &items,
            "Over 24 Hour Old Episode",
            over_24_season.id,
            over_24_series.id,
            over_24_season.id,
        )
        .await;
        let over_24_new_episode = create_episode(
            &items,
            "Over 24 Hour New Episode",
            over_24_season.id,
            over_24_series.id,
            over_24_season.id,
        )
        .await;
        let mut virtual_episode = NewBaseItem::new(Uuid::new_v4(), "Episode");
        virtual_episode.name = Some("Ignored Virtual Episode".to_owned());
        virtual_episode.sort_name = virtual_episode.name.clone();
        virtual_episode.parent_id = Some(over_24_season.id);
        virtual_episode.media_type = Some("Video".to_owned());
        virtual_episode.series_id = Some(over_24_series.id);
        virtual_episode.season_id = Some(over_24_season.id);
        virtual_episode.is_virtual_item = true;
        let virtual_episode = items
            .create(virtual_episode)
            .await
            .expect("virtual episode");

        let multi_season_series =
            create_item(&items, "Series", "Multi Season Series", parent.id).await;
        let multi_recent_season = create_item(
            &items,
            "Season",
            "Multi Series Recent Season",
            multi_season_series.id,
        )
        .await;
        let multi_old_season = create_item(
            &items,
            "Season",
            "Multi Series Old Season",
            multi_season_series.id,
        )
        .await;
        let multi_first_episode = create_episode(
            &items,
            "Multi Series First Recent Episode",
            multi_recent_season.id,
            multi_season_series.id,
            multi_recent_season.id,
        )
        .await;
        let multi_second_episode = create_episode(
            &items,
            "Multi Series Second Recent Episode",
            multi_recent_season.id,
            multi_season_series.id,
            multi_recent_season.id,
        )
        .await;
        let multi_old_episode = create_episode(
            &items,
            "Multi Series Old Episode",
            multi_old_season.id,
            multi_season_series.id,
            multi_old_season.id,
        )
        .await;

        let cross_season_series =
            create_item(&items, "Series", "Cross Season Series", parent.id).await;
        let cross_regular_season = create_item(
            &items,
            "Season",
            "Cross Regular Season",
            cross_season_series.id,
        )
        .await;
        let mut cross_special_season =
            create_item(&items, "Season", "Cross Specials", cross_season_series.id).await;
        cross_special_season.parent_index_number = Some(0);
        let cross_special_season = items
            .update(cross_special_season)
            .await
            .expect("special season marker");
        let cross_regular_episode = create_episode(
            &items,
            "Cross Regular Episode",
            cross_regular_season.id,
            cross_season_series.id,
            cross_regular_season.id,
        )
        .await;
        let cross_special_episode = create_episode(
            &items,
            "Cross Special Episode",
            cross_special_season.id,
            cross_season_series.id,
            cross_special_season.id,
        )
        .await;

        let hidden_series = create_item(&items, "Series", "Policy Hidden Series", parent.id).await;
        let hidden_season =
            create_item(&items, "Season", "Policy Hidden Season", hidden_series.id).await;
        let hidden_series_episode = create_episode(
            &items,
            "Visible Episode With Hidden Series",
            hidden_season.id,
            hidden_series.id,
            hidden_season.id,
        )
        .await;
        let saturated_parent =
            create_item(&items, "Folder", "Series Saturation Parent", root.id).await;
        let saturated_new_series = create_item(
            &items,
            "Series",
            "Series With Many New Episodes",
            saturated_parent.id,
        )
        .await;
        let saturated_new_season = create_item(
            &items,
            "Season",
            "Series With Many New Episodes Season",
            saturated_new_series.id,
        )
        .await;
        let mut saturated_new_episodes = Vec::new();
        for index in 1..=5 {
            saturated_new_episodes.push(
                create_episode(
                    &items,
                    &format!("Saturated New Episode {index}"),
                    saturated_new_season.id,
                    saturated_new_series.id,
                    saturated_new_season.id,
                )
                .await,
            );
        }
        let saturated_old_series = create_item(
            &items,
            "Series",
            "Series Hidden Beyond Episode Limit",
            saturated_parent.id,
        )
        .await;
        let saturated_old_season = create_item(
            &items,
            "Season",
            "Series Hidden Beyond Episode Limit Season",
            saturated_old_series.id,
        )
        .await;
        let saturated_old_episode = create_episode(
            &items,
            "Older Series Episode",
            saturated_old_season.id,
            saturated_old_series.id,
            saturated_old_season.id,
        )
        .await;
        let outer_album = create_item(&items, "MusicAlbum", "Outer Grouped Album", parent.id).await;
        let outer_album_folder =
            create_item(&items, "Folder", "Outer Album Folder", outer_album.id).await;
        let album = create_item(
            &items,
            "MusicAlbum",
            "Nearest Grouped Album",
            outer_album_folder.id,
        )
        .await;
        let album_folder = create_item(&items, "Folder", "Album Folder", album.id).await;
        let track = create_item(&items, "Audio", "Only Album Track", album_folder.id).await;
        let photo_album = create_item(&items, "PhotoAlbum", "Grouped Photo Album", parent.id).await;
        let photo_folder =
            create_item(&items, "Folder", "Grouped Photo Folder", photo_album.id).await;
        let first_photo = create_item(&items, "Photo", "First Photo", photo_folder.id).await;
        let second_photo = create_item(&items, "Photo", "Second Photo", photo_folder.id).await;
        let single_photo_album =
            create_item(&items, "PhotoAlbum", "Single Photo Album", parent.id).await;
        let single_photo_folder = create_item(
            &items,
            "Folder",
            "Single Photo Folder",
            single_photo_album.id,
        )
        .await;
        let single_photo = create_item(&items, "Photo", "Only Photo", single_photo_folder.id).await;
        let hidden_album =
            create_item(&items, "MusicAlbum", "Policy Hidden Album", parent.id).await;
        let hidden_album_folder =
            create_item(&items, "Folder", "Policy Hidden Folder", hidden_album.id).await;
        let hidden_album_track = create_item(
            &items,
            "Audio",
            "Visible Track With Hidden Album",
            hidden_album_folder.id,
        )
        .await;
        ItemValueRepository::new(database.clone())
            .link(
                hidden_album_track.id,
                item_value::ItemValueType::Tags,
                "Visible",
            )
            .await
            .expect("visible track tag");
        ItemValueRepository::new(database.clone())
            .link(
                hidden_series_episode.id,
                item_value::ItemValueType::Tags,
                "Visible",
            )
            .await
            .expect("visible episode tag");
        let restricted_policy = UserPolicy {
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            allowed_tags: vec!["Visible".to_owned()],
            ..UserPolicy::default()
        };
        users
            .update_policy(restricted_user.id, &restricted_policy)
            .await
            .expect("restricted user policy");
        let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
        alternate.name = Some("New Movie Alternate".to_owned());
        alternate.sort_name = alternate.name.clone();
        alternate.parent_id = Some(parent.id);
        alternate.media_type = Some("Video".to_owned());
        alternate.primary_version_id = Some(new_movie.id);
        let alternate = items
            .create(alternate)
            .await
            .expect("alternate movie version");
        set_date_created(&database, old_movie.id, 2026, 7, 22).await;
        set_date_created(&database, new_movie.id, 2026, 7, 24).await;
        set_date_created(&database, played_movie.id, 2026, 7, 25).await;
        set_date_created(&database, episode.id, 2026, 7, 26).await;
        set_date_created(&database, first_series_episode.id, 2026, 7, 20).await;
        set_date_created(&database, second_series_episode.id, 2026, 7, 21).await;
        set_date_created(&database, single_series_episode.id, 2026, 7, 18).await;
        set_date_created(&database, single_series_alternate.id, 2026, 7, 30).await;
        set_date_created(&database, over_24_old_episode.id, 2026, 7, 10).await;
        set_date_created(&database, over_24_new_episode.id, 2026, 7, 12).await;
        set_date_created(&database, virtual_episode.id, 2026, 7, 13).await;
        set_date_created(&database, multi_old_episode.id, 2026, 7, 10).await;
        set_date_created(&database, multi_first_episode.id, 2026, 7, 13).await;
        set_date_created(&database, multi_second_episode.id, 2026, 7, 14).await;
        set_date_created(&database, cross_regular_episode.id, 2026, 7, 15).await;
        set_date_created(&database, cross_special_episode.id, 2026, 7, 16).await;
        set_date_created(&database, hidden_series_episode.id, 2026, 7, 11).await;
        for episode in saturated_new_episodes {
            set_date_created(&database, episode.id, 2026, 8, 5).await;
        }
        set_date_created(&database, saturated_old_episode.id, 2026, 8, 4).await;
        set_date_created(&database, track.id, 2026, 7, 19).await;
        set_date_created(&database, first_photo.id, 2026, 7, 17).await;
        set_date_created(&database, second_photo.id, 2026, 7, 18).await;
        set_date_created(&database, single_photo.id, 2026, 7, 16).await;
        set_date_created(&database, hidden_album_track.id, 2026, 7, 15).await;
        set_date_created(&database, alternate.id, 2026, 7, 30).await;
        set_item_data(
            &database,
            new_movie.id,
            json!({ "DefaultPrimaryImageAspectRatio": 1.5 }),
        )
        .await;
        BaseItemImageRepository::new(database.clone())
            .replace(
                new_movie.id,
                &[
                    NewBaseItemImage {
                        image_type: BaseItemImageType::Primary,
                        image_index: 0,
                        path: "/media/new-movie-poster.jpg".to_owned(),
                        date_modified: Utc::now(),
                        width: Some(600),
                        height: Some(900),
                        blurhash: None,
                    },
                    NewBaseItemImage {
                        image_type: BaseItemImageType::Thumb,
                        image_index: 0,
                        path: "/media/new-movie-thumb.jpg".to_owned(),
                        date_modified: Utc::now(),
                        width: Some(1280),
                        height: Some(720),
                        blurhash: None,
                    },
                ],
            )
            .await
            .expect("new movie images");

        let mut played = NewUserData::new(played_movie.id, user.id, "latest-played");
        played.played = true;
        UserDataRepository::new(database.clone())
            .upsert(played)
            .await
            .expect("played user data");

        Self {
            app: jellyfin_api::router(AppState::new(
                database,
                "Latest Items Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
            restricted_user_token,
            parent_id: parent.id,
            old_movie_id: old_movie.id,
            new_movie_id: new_movie.id,
            played_movie_id: played_movie.id,
            episode_id: episode.id,
            series_id: series.id,
            first_series_episode_id: first_series_episode.id,
            second_series_episode_id: second_series_episode.id,
            single_series_id: single_series.id,
            over_24_series_id: over_24_series.id,
            over_24_new_episode_id: over_24_new_episode.id,
            multi_season_series_id: multi_season_series.id,
            multi_recent_season_id: multi_recent_season.id,
            cross_season_series_id: cross_season_series.id,
            hidden_series_episode_id: hidden_series_episode.id,
            saturated_parent_id: saturated_parent.id,
            saturated_new_series_id: saturated_new_series.id,
            saturated_old_series_id: saturated_old_series.id,
            album_id: album.id,
            album_track_id: track.id,
            photo_album_id: photo_album.id,
            first_photo_id: first_photo.id,
            second_photo_id: second_photo.id,
            single_photo_album_id: single_photo_album.id,
            single_photo_id: single_photo.id,
            hidden_album_track_id: hidden_album_track.id,
        }
    }
}

async fn assert_auth_and_target_user_rules(fixture: &Fixture) {
    assert_eq!(
        request(&fixture.app, "/Items/Latest", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &fixture.app,
            &format!("/Items/Latest?userId={}", fixture.other_user_id),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.app,
            &format!("/Users/{}/Items/Latest", Uuid::new_v4()),
            Some(&fixture.admin_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_latest_defaults_hide_played_and_sort_by_created(fixture: &Fixture) {
    let latest = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&limit=2",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    let latest_items = latest.as_array().unwrap();
    assert_eq!(latest_items.len(), 2);
    assert_eq!(latest[0]["Id"], fixture.new_movie_id.simple().to_string());
    assert_eq!(latest[1]["Id"], fixture.old_movie_id.simple().to_string());
    assert!(latest_items.iter().all(|item| item["Type"] == "Movie"));
    assert!(latest.get("Items").is_none());
}

async fn assert_is_played_and_legacy_routes(fixture: &Fixture) {
    let played = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=true",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(played.as_array().unwrap().len(), 1);
    assert_eq!(
        played[0]["Id"],
        fixture.played_movie_id.simple().to_string()
    );

    let mixed = get_json(
        &fixture.app,
        &format!(
            "/Users/{}/Items/Latest?parentId={}&includeItemTypes=Movie,Episode&isPlayed=false&groupItems=false&limit=2",
            fixture.user_id, fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(mixed.as_array().unwrap().len(), 2);
    assert_eq!(mixed[0]["Id"], fixture.episode_id.simple().to_string());
    assert_eq!(mixed[1]["Id"], fixture.new_movie_id.simple().to_string());
}

async fn assert_default_grouping_and_explicit_ungrouping(fixture: &Fixture) {
    let grouped = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=20",
            fixture.series_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(grouped.as_array().unwrap().len(), 1);
    assert_eq!(grouped[0]["Id"], fixture.series_id.simple().to_string());
    assert_eq!(grouped[0]["Type"], "Series");
    assert_eq!(grouped[0]["ChildCount"], 2);

    let singleton = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=20",
            fixture.single_series_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(singleton.as_array().unwrap().len(), 1);
    assert_eq!(
        singleton[0]["Id"],
        fixture.single_series_id.simple().to_string()
    );
    assert_eq!(singleton[0]["Type"], "Series");
    assert_eq!(singleton[0]["ChildCount"], 1);

    let limited = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(limited.as_array().unwrap().len(), 1);
    assert_eq!(limited[0]["Id"], fixture.episode_id.simple().to_string());

    for group_items in ["groupItems", "GroupItems", "groupitems"] {
        let ungrouped = get_json(
            &fixture.app,
            &format!(
                "/Items/Latest?parentId={}&includeItemTypes=Episode&{group_items}=false&limit=20",
                fixture.series_id
            ),
            &fixture.user_token,
        )
        .await;
        assert_eq!(ungrouped.as_array().unwrap().len(), 2, "{group_items}");
        assert_eq!(
            ungrouped[0]["Id"],
            fixture.second_series_episode_id.simple().to_string(),
            "{group_items}"
        );
        assert_eq!(
            ungrouped[1]["Id"],
            fixture.first_series_episode_id.simple().to_string(),
            "{group_items}"
        );
    }

    let album = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Audio&limit=20",
            fixture.album_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(album.as_array().unwrap().len(), 1);
    assert_eq!(album[0]["Id"], fixture.album_id.simple().to_string());
    assert_eq!(album[0]["Type"], "MusicAlbum");
    assert_eq!(album[0]["ChildCount"], 1);
}

async fn assert_tv_latest_window_grouping(fixture: &Fixture) {
    let outside_window = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=20",
            fixture.over_24_series_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(outside_window.as_array().unwrap().len(), 1);
    assert_eq!(
        outside_window[0]["Id"],
        fixture.over_24_new_episode_id.simple().to_string()
    );
    assert_eq!(outside_window[0]["Type"], "Episode");
    assert!(outside_window[0].get("ChildCount").is_none());

    let one_recent_season = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=20",
            fixture.multi_season_series_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(one_recent_season.as_array().unwrap().len(), 1);
    assert_eq!(
        one_recent_season[0]["Id"],
        fixture.multi_recent_season_id.simple().to_string()
    );
    assert_eq!(one_recent_season[0]["Type"], "Season");
    assert_eq!(one_recent_season[0]["ChildCount"], 2);

    let across_seasons = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=20",
            fixture.cross_season_series_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(across_seasons.as_array().unwrap().len(), 1);
    assert_eq!(
        across_seasons[0]["Id"],
        fixture.cross_season_series_id.simple().to_string()
    );
    assert_eq!(across_seasons[0]["Type"], "Series");
    assert_eq!(across_seasons[0]["ChildCount"], 2);

    let hidden_container = get_json(
        &fixture.app,
        "/Items/Latest?includeItemTypes=Episode&limit=20",
        &fixture.restricted_user_token,
    )
    .await;
    assert_eq!(hidden_container.as_array().unwrap().len(), 1);
    assert_eq!(
        hidden_container[0]["Id"],
        fixture.hidden_series_episode_id.simple().to_string()
    );
    assert_eq!(hidden_container[0]["Type"], "Episode");
    assert!(hidden_container[0].get("ChildCount").is_none());

    let saturated = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&limit=2",
            fixture.saturated_parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(saturated.as_array().unwrap().len(), 2);
    assert_eq!(
        saturated[0]["Id"],
        fixture.saturated_new_series_id.simple().to_string()
    );
    assert_eq!(
        saturated[1]["Id"],
        fixture.saturated_old_series_id.simple().to_string()
    );
    assert!(
        saturated
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Series")
    );

    let ungrouped = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Episode&groupItems=false&limit=20",
            fixture.multi_season_series_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(ungrouped.as_array().unwrap().len(), 3);
    assert!(
        ungrouped
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Episode" && item.get("ChildCount").is_none())
    );
}

async fn assert_album_ancestor_grouping(fixture: &Fixture) {
    let ungrouped_audio = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Audio&groupItems=false&limit=20",
            fixture.album_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(ungrouped_audio.as_array().unwrap().len(), 1);
    assert_eq!(
        ungrouped_audio[0]["Id"],
        fixture.album_track_id.simple().to_string()
    );
    assert_eq!(ungrouped_audio[0]["Type"], "Audio");
    assert!(ungrouped_audio[0].get("ChildCount").is_none());

    let grouped_photos = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Photo&limit=20",
            fixture.photo_album_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(grouped_photos.as_array().unwrap().len(), 1);
    assert_eq!(
        grouped_photos[0]["Id"],
        fixture.photo_album_id.simple().to_string()
    );
    assert_eq!(grouped_photos[0]["Type"], "PhotoAlbum");
    assert_eq!(grouped_photos[0]["ChildCount"], 2);

    let ungrouped_photos = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Photo&groupItems=false&limit=20",
            fixture.photo_album_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(ungrouped_photos.as_array().unwrap().len(), 2);
    assert_eq!(
        ungrouped_photos[0]["Id"],
        fixture.second_photo_id.simple().to_string()
    );
    assert_eq!(
        ungrouped_photos[1]["Id"],
        fixture.first_photo_id.simple().to_string()
    );
    assert!(
        ungrouped_photos
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Photo" && item.get("ChildCount").is_none())
    );

    let single_photo = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Photo&limit=20",
            fixture.single_photo_album_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(single_photo.as_array().unwrap().len(), 1);
    assert_eq!(
        single_photo[0]["Id"],
        fixture.single_photo_id.simple().to_string()
    );
    assert_eq!(single_photo[0]["Type"], "Photo");
    assert!(single_photo[0].get("ChildCount").is_none());

    let hidden_container_fallback = get_json(
        &fixture.app,
        "/Items/Latest?includeItemTypes=Audio&limit=20",
        &fixture.restricted_user_token,
    )
    .await;
    assert_eq!(hidden_container_fallback.as_array().unwrap().len(), 1);
    assert_eq!(
        hidden_container_fallback[0]["Id"],
        fixture.hidden_album_track_id.simple().to_string()
    );
    assert_eq!(hidden_container_fallback[0]["Type"], "Audio");
    assert!(hidden_container_fallback[0].get("ChildCount").is_none());
}

async fn assert_latest_dto_options_and_image_fields(fixture: &Fixture) {
    let defaults = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&limit=2",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    let newest = defaults
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == fixture.new_movie_id.simple().to_string())
        .expect("new movie in latest results");
    assert!(newest["ImageTags"]["Primary"].is_string());
    assert!(newest["UserData"].is_object());

    let disabled = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&fields=PrimaryImageAspectRatio,MediaSourceCount&enableImages=false&enableUserData=false&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(disabled.as_array().unwrap().len(), 1);
    assert_eq!(disabled[0]["Id"], fixture.new_movie_id.simple().to_string());
    assert_eq!(disabled[0]["PrimaryImageAspectRatio"], 1.5);
    assert_eq!(disabled[0]["MediaSourceCount"], 2);
    assert!(disabled[0].get("ImageTags").is_none());
    assert!(disabled[0].get("UserData").is_none());

    let primary_by_number = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&enableImageTypes=0&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(primary_by_number[0]["ImageTags"]["Primary"].is_string());
    assert!(primary_by_number[0]["ImageTags"].get("Thumb").is_none());

    let thumb_by_number = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&enableImageTypes=5&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(thumb_by_number[0]["ImageTags"]["Thumb"].is_string());
    assert!(thumb_by_number[0]["ImageTags"].get("Primary").is_none());

    let mixed_repeated = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&enableImageTypes=pRiMaRy%2Cinvalid&enableImageTypes=5&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(mixed_repeated[0]["ImageTags"]["Primary"].is_string());
    assert!(mixed_repeated[0]["ImageTags"]["Thumb"].is_string());

    let names_case_insensitive = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&enableImageTypes=pRiMaRy%2CtHuMb&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(names_case_insensitive[0]["ImageTags"]["Primary"].is_string());
    assert!(names_case_insensitive[0]["ImageTags"]["Thumb"].is_string());

    let invalid_text = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&enableImageTypes=invalid&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(invalid_text[0]["ImageTags"]["Primary"].is_string());
    assert!(invalid_text[0]["ImageTags"]["Thumb"].is_string());

    let unknown_number = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&enableImageTypes=99&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(unknown_number[0].get("ImageTags").is_none());
    assert!(unknown_number[0].get("PrimaryImageTag").is_none());

    let no_images = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?parentId={}&includeItemTypes=Movie&isPlayed=false&imageTypeLimit=0&limit=1",
            fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(no_images[0].get("ImageTags").is_none());
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let response = request(app, uri, Some(token)).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    item.is_folder = matches!(
        item_type,
        "Folder" | "Series" | "Season" | "MusicAlbum" | "PhotoAlbum"
    );
    item.media_type = match item_type {
        "Audio" => Some("Audio".to_owned()),
        "Photo" => Some("Photo".to_owned()),
        _ if !item.is_folder => Some("Video".to_owned()),
        _ => None,
    };
    repository.create(item).await.expect("base item")
}

async fn create_episode(
    repository: &BaseItemRepository,
    name: &str,
    parent_id: Uuid,
    series_id: Uuid,
    season_id: Uuid,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Episode");
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.media_type = Some("Video".to_owned());
    item.series_id = Some(series_id);
    item.season_id = Some(season_id);
    repository.create(item).await.expect("episode")
}

async fn set_item_data(database: &DatabaseConnection, item_id: Uuid, data: Value) {
    base_item::Entity::update_many()
        .col_expr(base_item::Column::Data, Expr::value(data))
        .filter(base_item::Column::Id.eq(item_id))
        .exec(database)
        .await
        .expect("item data update");
}

async fn set_date_created(
    database: &DatabaseConnection,
    item_id: Uuid,
    year: i32,
    month: u32,
    day: u32,
) {
    base_item::Entity::update_many()
        .col_expr(
            base_item::Column::DateCreated,
            Expr::value(Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()),
        )
        .filter(base_item::Column::Id.eq(item_id))
        .exec(database)
        .await
        .expect("date_created update");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Latest Items Tests",
            "1.0",
            "Test",
            format!("latest-items-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
