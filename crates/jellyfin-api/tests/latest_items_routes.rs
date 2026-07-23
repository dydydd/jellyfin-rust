use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice, NewUserData,
    UserDataRepository, entities::base_item,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, sea_query::Expr,
};
use serde_json::Value;
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

    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    parent_id: Uuid,
    old_movie_id: Uuid,
    new_movie_id: Uuid,
    played_movie_id: Uuid,
    episode_id: Uuid,
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
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("root");
        let parent = create_item(&items, "Folder", "Latest Parent", root.id).await;
        let old_movie = create_item(&items, "Movie", "Old Movie", parent.id).await;
        let new_movie = create_item(&items, "Movie", "New Movie", parent.id).await;
        let played_movie = create_item(&items, "Movie", "Played Movie", parent.id).await;
        let episode = create_item(&items, "Episode", "Newest Episode", parent.id).await;
        set_date_created(&database, old_movie.id, 2026, 7, 22).await;
        set_date_created(&database, new_movie.id, 2026, 7, 24).await;
        set_date_created(&database, played_movie.id, 2026, 7, 25).await;
        set_date_created(&database, episode.id, 2026, 7, 26).await;

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
            parent_id: parent.id,
            old_movie_id: old_movie.id,
            new_movie_id: new_movie.id,
            played_movie_id: played_movie.id,
            episode_id: episode.id,
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
            "/Users/{}/Items/Latest?parentId={}&includeItemTypes=Movie,Episode&isPlayed=false&limit=2",
            fixture.user_id, fixture.parent_id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(mixed.as_array().unwrap().len(), 2);
    assert_eq!(mixed[0]["Id"], fixture.episode_id.simple().to_string());
    assert_eq!(mixed[1]["Id"], fixture.new_movie_id.simple().to_string());
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
    item.is_folder = item_type == "Folder";
    item.media_type = (!item.is_folder).then(|| "Video".to_owned());
    repository.create(item).await.expect("base item")
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
