use std::collections::BTreeMap;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    ServerConfigurationRepository,
};
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_item_content_type_";

#[tokio::test]
async fn item_content_type_route_is_elevated_atomic_and_persistent() {
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
        exercise_routes(&task_database_name).await;
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

async fn exercise_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("content-type-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let ordinary_user = users
        .create(&format!("content-type-user-{suffix}"))
        .await
        .expect("ordinary user creation");
    let devices = DeviceRepository::new(database.clone());
    let administrator_token =
        create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = create_session(&devices, ordinary_user.id, &format!("user-{suffix}")).await;

    let items = BaseItemRepository::new(database.clone());
    let movies = create_item(&items, "/Media/Movies", true).await;
    let movies_case_variant = create_item(&items, "/media/movies", true).await;
    let episode = create_item(&items, "/Media/Shows/episode.mkv", false).await;
    let music = create_item(&items, "/Media/Music", true).await;
    let route_app = app(database.clone());

    assert_access(&route_app, movies, &user_token, &administrator_token).await;
    assert_case_insensitive_replacement_and_removal(
        &database,
        &route_app,
        movies,
        movies_case_variant,
        &administrator_token,
    )
    .await;
    assert_restart_and_concurrent_updates(
        &database,
        &route_app,
        movies,
        episode,
        music,
        &administrator_token,
    )
    .await;

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_access(route_app: &Router, movies: Uuid, user_token: &str, admin_token: &str) {
    assert_eq!(
        post(route_app, movies, "movies", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(route_app, movies, "movies", Some(user_token)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(route_app, Uuid::new_v4(), "movies", Some(admin_token),).await,
        StatusCode::NOT_FOUND
    );
}

async fn assert_case_insensitive_replacement_and_removal(
    database: &sea_orm::DatabaseConnection,
    route_app: &Router,
    movies: Uuid,
    movies_case_variant: Uuid,
    administrator_token: &str,
) {
    assert_eq!(
        post_raw(
            route_app,
            &format!("/Items/{movies}/ContentType?ContentType=movies"),
            Some(administrator_token),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(
            route_app,
            movies_case_variant,
            "tvshows",
            Some(administrator_token),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        persisted_content_types(database).await,
        BTreeMap::from([("/media/movies".to_owned(), "tvshows".to_owned())])
    );
    assert_eq!(
        post_raw(
            route_app,
            &format!("/Items/{movies_case_variant}/ContentType?contentType=%20%20"),
            Some(administrator_token),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert!(persisted_content_types(database).await.is_empty());
}

async fn assert_restart_and_concurrent_updates(
    database: &sea_orm::DatabaseConnection,
    route_app: &Router,
    movies: Uuid,
    episode: Uuid,
    music: Uuid,
    administrator_token: &str,
) {
    assert_eq!(
        post(route_app, movies, "movies", Some(administrator_token)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_raw(
            route_app,
            &format!("/Items/{movies}/ContentType"),
            Some(administrator_token),
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert!(persisted_content_types(database).await.is_empty());
    assert_eq!(
        post(route_app, movies, "movies", Some(administrator_token)).await,
        StatusCode::NO_CONTENT
    );
    let restarted_app = app(database.clone());
    let (shows_status, music_status) = tokio::join!(
        post(
            &restarted_app,
            episode,
            "tvshows",
            Some(administrator_token),
        ),
        post(&restarted_app, music, "music", Some(administrator_token)),
    );
    assert_eq!(shows_status, StatusCode::NO_CONTENT);
    assert_eq!(music_status, StatusCode::NO_CONTENT);
    assert_eq!(
        persisted_content_types(database).await,
        BTreeMap::from([
            ("/Media/Movies".to_owned(), "movies".to_owned()),
            ("/Media/Music".to_owned(), "music".to_owned()),
            ("/Media/Shows".to_owned(), "tvshows".to_owned()),
        ])
    );
}

fn app(database: sea_orm::DatabaseConnection) -> Router {
    jellyfin_api::router(AppState::new(
        database,
        "Item Content Type Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ))
}

async fn create_item(repository: &BaseItemRepository, path: &str, is_folder: bool) -> Uuid {
    let mut item = NewBaseItem::new(Uuid::new_v4(), if is_folder { "Folder" } else { "Episode" });
    item.path = Some(path.to_owned());
    item.is_folder = is_folder;
    repository
        .create(item)
        .await
        .expect("base item creation")
        .id
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Item Content Type Tests",
            "1.0",
            "PostgreSQL",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn post(app: &Router, item_id: Uuid, content_type: &str, token: Option<&str>) -> StatusCode {
    post_raw(
        app,
        &format!("/Items/{item_id}/ContentType?contentType={content_type}"),
        token,
    )
    .await
}

async fn post_raw(app: &Router, uri: &str, token: Option<&str>) -> StatusCode {
    let mut request = Request::post(uri);
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

async fn persisted_content_types(
    database: &sea_orm::DatabaseConnection,
) -> BTreeMap<String, String> {
    let configuration = ServerConfigurationRepository::new(database.clone())
        .load()
        .await
        .expect("server configuration load");
    content_types(&configuration.content_types)
}

fn content_types(value: &Value) -> BTreeMap<String, String> {
    value
        .as_array()
        .expect("content types must be a JSON array")
        .iter()
        .map(|entry| {
            (
                entry["Name"]
                    .as_str()
                    .expect("content-type name")
                    .to_owned(),
                entry["Value"]
                    .as_str()
                    .expect("content-type value")
                    .to_owned(),
            )
        })
        .collect()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
