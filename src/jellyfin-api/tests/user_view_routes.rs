use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{UserService, VirtualFolderService};
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewUserData, USER_ROOT_FOLDER_ID,
    UserDataRepository,
    entities::{user, user::Column as UserColumn},
};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User View Tests\", DeviceId=\"user-view-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_user_view_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;
const USER_VIEW_DISPLAY_PREFERENCES_ID: &str = "cb46bc72e78d95cc6cd072de3a65b93a";

#[tokio::test]
async fn user_views_and_grouping_options_follow_official_contract() {
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
        exercise_user_view_routes(&task_database_name).await;
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

async fn exercise_user_view_routes(database_name: &str) {
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
    assert_user_views(&fixture).await;
    assert_home_query_key_casing(&fixture).await;
    assert_grouped_views(&fixture).await;
    assert_grouping_options(&fixture).await;
    database.close().await.expect("database pool cleanup");
}

async fn assert_home_query_key_casing(fixture: &Fixture) {
    let views = get_json(
        &fixture.app,
        &format!("/UserViews?userid={}&presetviews=movies", fixture.user_id),
        &fixture.user_token,
    )
    .await;
    let synthetic_id = views["Items"]
        .as_array()
        .expect("view items")
        .iter()
        .find(|item| item["CollectionType"] == "movies")
        .and_then(|item| item["Id"].as_str())
        .expect("synthetic movie view");
    let items = get_json(
        &fixture.app,
        &format!(
            "/Items?parentid={synthetic_id}&Recursive=true&includeitemtypes=Movie&Limit=1&Ids={}",
            fixture.movie_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(items["TotalRecordCount"], 1);
    assert_eq!(
        items["Items"][0]["Id"],
        fixture.movie_id.simple().to_string()
    );

    let latest = get_json(
        &fixture.app,
        &format!(
            "/Items/Latest?userid={}&parentid={synthetic_id}&includeitemtypes=Movie&Limit=1",
            fixture.user_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(latest.as_array().expect("latest items").len(), 1);
    assert_eq!(latest[0]["Id"], fixture.movie_id.simple().to_string());
}

struct Fixture {
    app: axum::Router,
    database: DatabaseConnection,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    movie_view_id: Uuid,
    show_view_id: Uuid,
    mixed_view_id: Uuid,
    movie_id: Uuid,
    mixed_movie_id: Uuid,
    ungrouped_movie_id: Uuid,
    show_id: Uuid,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("user-view-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("user-view-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("user-view-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let virtual_folders = VirtualFolderService::new(database.clone());
        virtual_folders
            .create(
                &format!("Movies {suffix}"),
                Some("movies".to_owned()),
                json!({ "Enabled": true }),
                Vec::new(),
                false,
            )
            .await
            .expect("movie view");
        virtual_folders
            .create(
                &format!("Shows {suffix}"),
                Some("tvshows".to_owned()),
                json!({ "Enabled": true }),
                Vec::new(),
                false,
            )
            .await
            .expect("tv view");
        virtual_folders
            .create(
                &format!("Music {suffix}"),
                Some("music".to_owned()),
                json!({ "Enabled": true }),
                Vec::new(),
                false,
            )
            .await
            .expect("music view");
        virtual_folders
            .create(
                &format!("Mixed {suffix}"),
                None,
                json!({ "Enabled": true }),
                Vec::new(),
                false,
            )
            .await
            .expect("mixed view");
        virtual_folders
            .create(
                &format!("Other Movies {suffix}"),
                Some("movies".to_owned()),
                json!({ "Enabled": true }),
                Vec::new(),
                false,
            )
            .await
            .expect("ungrouped movie view");
        virtual_folders
            .create(
                &format!("Hidden {suffix}"),
                Some("movies".to_owned()),
                json!({ "IsHidden": true }),
                Vec::new(),
                false,
            )
            .await
            .expect("hidden view");
        let views = virtual_folders.list().await.expect("view list");
        let id_by_name = |prefix: &str| -> Uuid {
            views
                .iter()
                .find(|view| view.name.starts_with(prefix))
                .expect("view id")
                .id
        };
        let movie_view_id = id_by_name("Movies");
        let show_view_id = id_by_name("Shows");
        let mixed_view_id = id_by_name("Mixed");
        let ungrouped_movie_view_id = id_by_name("Other Movies");
        let items = BaseItemRepository::new(database.clone());
        items.ensure_user_root().await.expect("user root");
        for (id, name, collection_type) in [
            (movie_view_id, format!("Movies {suffix}"), Some("movies")),
            (show_view_id, format!("Shows {suffix}"), Some("tvshows")),
            (mixed_view_id, format!("Mixed {suffix}"), None),
            (
                ungrouped_movie_view_id,
                format!("Other Movies {suffix}"),
                Some("movies"),
            ),
        ] {
            let mut collection = NewBaseItem::new(id, "CollectionFolder");
            collection.parent_id = Some(USER_ROOT_FOLDER_ID);
            collection.name = Some(name);
            collection.sort_name = collection.name.clone();
            collection.is_folder = true;
            collection.data = Some(if id == movie_view_id {
                json!({
                    "CollectionType": collection_type,
                    "DefaultPrimaryImageAspectRatio": 1.5
                })
            } else {
                json!({ "CollectionType": collection_type })
            });
            items.create(collection).await.expect("collection item");
        }
        BaseItemImageRepository::new(database.clone())
            .set_or_append(
                movie_view_id,
                NewBaseItemImage {
                    image_type: BaseItemImageType::Primary,
                    image_index: 0,
                    path: format!("/metadata/{movie_view_id}/folder.jpg"),
                    date_modified: chrono::Utc::now(),
                    width: Some(600),
                    height: Some(400),
                    blurhash: None,
                },
            )
            .await
            .expect("movie collection image");
        let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        movie.parent_id = Some(movie_view_id);
        movie.name = Some(format!("Movie {suffix}"));
        movie.sort_name = movie.name.clone();
        movie.media_type = Some("Video".to_owned());
        let movie = items.create(movie).await.expect("movie item");
        let mixed_movie = create_media_item(&items, mixed_view_id, "Movie", &suffix).await;
        let ungrouped_movie =
            create_media_item(&items, ungrouped_movie_view_id, "Movie", &suffix).await;
        let show = create_media_item(&items, show_view_id, "Series", &suffix).await;
        let user_data = UserDataRepository::new(database.clone());
        for item_id in [movie.id, mixed_movie.id, ungrouped_movie.id] {
            let mut data = NewUserData::new(item_id, user.id, item_id.to_string());
            data.playback_position_ticks = 10_000_000;
            data.last_played_date = Some(chrono::Utc::now());
            user_data.upsert(data).await.expect("resume data");
        }

        Self {
            app: jellyfin_api::router(AppState::new(
                database.clone(),
                "User View Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
            movie_view_id,
            show_view_id,
            mixed_view_id,
            movie_id: movie.id,
            mixed_movie_id: mixed_movie.id,
            ungrouped_movie_id: ungrouped_movie.id,
            show_id: show.id,
        }
    }
}

async fn create_media_item(
    items: &BaseItemRepository,
    parent_id: Uuid,
    item_type: &str,
    suffix: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = Some(parent_id);
    item.name = Some(format!("{item_type} {suffix} {parent_id}"));
    item.sort_name = item.name.clone();
    item.media_type = Some("Video".to_owned());
    items.create(item).await.expect("media item")
}

async fn assert_auth_and_target_user_rules(fixture: &Fixture) {
    assert_eq!(
        request(&fixture.app, "/UserViews", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &fixture.app,
            &format!("/UserViews?userId={}", fixture.other_user_id),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.app,
            &format!("/Users/{}/Views", Uuid::new_v4()),
            Some(&fixture.admin_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_user_views(fixture: &Fixture) {
    let views = get_json(&fixture.app, "/UserViews", &fixture.user_token).await;
    assert_eq!(views["StartIndex"], 0);
    assert_eq!(views["TotalRecordCount"], 5);
    assert!(views.get("total_record_count").is_none());
    let items = views["Items"].as_array().expect("view items");
    assert!(items.iter().all(|item| item["Type"] == "CollectionFolder"));
    assert!(items.iter().all(|item| item["IsFolder"] == true));
    assert!(items.iter().any(
        |item| item["Id"] == fixture.movie_view_id.simple().to_string()
            && item["CollectionType"] == "movies"
    ));
    let movie_collection = items
        .iter()
        .find(|item| item["Id"] == fixture.movie_view_id.simple().to_string())
        .expect("movie collection DTO");
    assert_eq!(movie_collection["ChildCount"], 1);
    assert_eq!(
        movie_collection["DisplayPreferencesId"],
        fixture.movie_view_id.simple().to_string()
    );
    assert_eq!(movie_collection["PrimaryImageAspectRatio"], 1.5);
    assert!(movie_collection["ImageTags"]["Primary"].is_string());

    let movie_views = get_json(
        &fixture.app,
        &format!("/UserViews?userId={}&presetViews=movies", fixture.user_id),
        &fixture.admin_token,
    )
    .await;
    assert_eq!(movie_views["TotalRecordCount"], 5);
    let movie_view = movie_views["Items"]
        .as_array()
        .expect("movie preset view items")
        .iter()
        .find(|item| item["CollectionType"] == "movies")
        .expect("movie preset view");
    assert_eq!(movie_view["Type"], "UserView");
    assert_ne!(movie_view["Id"], fixture.movie_view_id.simple().to_string());
    assert_eq!(movie_view["ChildCount"], 1);
    assert_eq!(
        movie_view["DisplayPreferencesId"],
        USER_VIEW_DISPLAY_PREFERENCES_ID
    );
    let persisted = get_json(
        &fixture.app,
        &format!("/Items/{}", movie_view["Id"].as_str().unwrap()),
        &fixture.user_token,
    )
    .await;
    assert_eq!(persisted["Type"], "UserView");
    assert_eq!(persisted["CollectionType"], "movies");
    assert_eq!(persisted["IsVirtualItem"], true);

    let synthetic_id = movie_view["Id"].as_str().expect("synthetic view id");
    let contents = get_json(
        &fixture.app,
        &format!("/Items?parentId={synthetic_id}&recursive=true&includeItemTypes=Movie"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(contents["TotalRecordCount"], 1);
    assert_eq!(
        contents["Items"][0]["Id"],
        fixture.movie_id.simple().to_string()
    );

    let latest = get_json(
        &fixture.app,
        &format!("/Items/Latest?parentId={synthetic_id}&includeItemTypes=Movie&limit=1"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(latest.as_array().expect("latest items").len(), 1);
    assert_eq!(latest[0]["Id"], fixture.movie_id.simple().to_string());

    let with_hidden = get_json(
        &fixture.app,
        &format!("/Users/{}/Views?includeHidden=true", fixture.user_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(with_hidden["TotalRecordCount"], 6);
}

async fn assert_grouped_views(fixture: &Fixture) {
    set_grouped_folders(
        &fixture.database,
        fixture.user_id,
        &[
            fixture.movie_view_id,
            fixture.show_view_id,
            fixture.mixed_view_id,
        ],
    )
    .await;

    let views = get_json(&fixture.app, "/UserViews", &fixture.user_token).await;
    assert_eq!(views["TotalRecordCount"], 4);
    let items = views["Items"].as_array().expect("grouped view items");
    let movies = items
        .iter()
        .find(|item| item["CollectionType"] == "movies")
        .expect("grouped movie view");
    assert_eq!(movies["Type"], "UserView");
    assert_eq!(movies["Name"], "Movies");
    assert_eq!(movies["IsFolder"], true);
    assert_eq!(movies["ChildCount"], 2);
    assert_eq!(
        movies["DisplayPreferencesId"],
        USER_VIEW_DISPLAY_PREFERENCES_ID
    );
    let tvshows = items
        .iter()
        .find(|item| item["CollectionType"] == "tvshows")
        .expect("grouped television view");
    assert_eq!(tvshows["Type"], "UserView");
    assert_eq!(tvshows["Name"], "TvShows");
    assert!(items.iter().any(|item| item["CollectionType"] == "music"));

    let movie_view_id = movies["Id"].as_str().expect("grouped movie id");
    let movie_contents = get_json(
        &fixture.app,
        &format!("/Items?parentId={movie_view_id}&recursive=true&includeItemTypes=Movie&limit=20"),
        &fixture.user_token,
    )
    .await;
    let movie_ids = movie_contents["Items"]
        .as_array()
        .expect("grouped movie items")
        .iter()
        .filter_map(|item| item["Id"].as_str())
        .collect::<Vec<_>>();
    let movie_id = fixture.movie_id.simple().to_string();
    let mixed_movie_id = fixture.mixed_movie_id.simple().to_string();
    let ungrouped_movie_id = fixture.ungrouped_movie_id.simple().to_string();
    let show_id = fixture.show_id.simple().to_string();
    assert_eq!(movie_contents["TotalRecordCount"], 2);
    assert!(movie_ids.contains(&movie_id.as_str()));
    assert!(movie_ids.contains(&mixed_movie_id.as_str()));
    assert!(!movie_ids.contains(&ungrouped_movie_id.as_str()));
    assert!(!movie_ids.contains(&show_id.as_str()));

    let latest = get_json(
        &fixture.app,
        &format!("/Items/Latest?parentId={movie_view_id}&includeItemTypes=Movie&limit=20"),
        &fixture.user_token,
    )
    .await;
    let latest_ids = latest
        .as_array()
        .expect("grouped latest items")
        .iter()
        .filter_map(|item| item["Id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(latest_ids.len(), 2);
    assert!(latest_ids.contains(&movie_id.as_str()));
    assert!(latest_ids.contains(&mixed_movie_id.as_str()));
    assert!(!latest_ids.contains(&ungrouped_movie_id.as_str()));

    let resume = get_json(
        &fixture.app,
        &format!(
            "/Users/{}/Items/Resume?parentId={movie_view_id}&includeItemTypes=Movie&limit=20",
            fixture.user_id
        ),
        &fixture.user_token,
    )
    .await;
    let resume_ids = resume["Items"]
        .as_array()
        .expect("grouped resume items")
        .iter()
        .filter_map(|item| item["Id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(resume["TotalRecordCount"], 2);
    assert!(resume_ids.contains(&movie_id.as_str()));
    assert!(resume_ids.contains(&mixed_movie_id.as_str()));
    assert!(!resume_ids.contains(&ungrouped_movie_id.as_str()));
}

async fn assert_grouping_options(fixture: &Fixture) {
    let grouping = get_json(
        &fixture.app,
        &format!("/UserViews/GroupingOptions?userId={}", fixture.user_id),
        &fixture.admin_token,
    )
    .await;
    let items = grouping.as_array().expect("grouping array");
    assert_eq!(items.len(), 5);
    assert!(
        items
            .iter()
            .any(|item| item["Id"] == fixture.movie_view_id.simple().to_string())
    );
    assert!(
        items
            .iter()
            .any(|item| item["Id"] == fixture.show_view_id.simple().to_string())
    );
    assert!(
        items
            .iter()
            .any(|item| item["Id"] == fixture.mixed_view_id.simple().to_string())
    );

    let legacy = get_json(
        &fixture.app,
        &format!("/Users/{}/GroupingOptions", fixture.user_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(legacy, grouping);
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

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "User View Tests",
            "1.0",
            "Test",
            format!("user-view-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn set_grouped_folders(database: &DatabaseConnection, user_id: Uuid, folder_ids: &[Uuid]) {
    let preferences = serde_json::json!({
        "GroupedFolders": folder_ids
            .iter()
            .map(|id| id.simple().to_string())
            .collect::<Vec<_>>()
    });
    user::Entity::update_many()
        .col_expr(
            user::Column::Preferences,
            sea_orm::sea_query::Expr::value(preferences),
        )
        .col_expr(
            user::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now()),
        )
        .filter(UserColumn::Id.eq(user_id))
        .exec(database)
        .await
        .expect("grouped folders preference update");
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
