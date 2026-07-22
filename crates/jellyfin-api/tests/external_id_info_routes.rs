use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_external_id_infos_";
const MAX_RESPONSE_SIZE: usize = 64 * 1024;

#[tokio::test]
async fn external_id_infos_route_is_elevated_persisted_and_item_specific() {
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
        exercise_route(&task_database_name).await;
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

async fn exercise_route(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 6,
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
        .create_initial_administrator(&format!("external-id-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let ordinary = users
        .create(&format!("external-id-user-{suffix}"))
        .await
        .expect("ordinary user creation");
    let devices = DeviceRepository::new(database.clone());
    let administrator_token =
        create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let ordinary_token = create_session(&devices, ordinary.id, &format!("user-{suffix}")).await;
    let movie = BaseItemRepository::new(database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("movie creation");
    let route_app = app(database.clone());

    assert_eq!(
        get(&route_app, movie.id, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&route_app, movie.id, Some(&ordinary_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&route_app, Uuid::new_v4(), Some(&administrator_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let response = get(&route_app, movie.id, Some(&administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let expected = json!([
        { "Name": "IMDb", "Key": "Imdb" },
        { "Name": "TheMovieDb", "Key": "Tmdb", "Type": "Movie" },
        { "Name": "TheMovieDb", "Key": "TmdbCollection", "Type": "BoxSet" }
    ]);
    assert_eq!(body_json(response).await, expected);

    let restarted = app(database.clone());
    let response = get(&restarted, movie.id, Some(&administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, expected);

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

fn app(database: sea_orm::DatabaseConnection) -> Router {
    jellyfin_api::router(AppState::new(
        database,
        "External Id Info Test".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ))
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "External Id Info Tests",
            "1.0",
            "PostgreSQL",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn get(app: &Router, item_id: Uuid, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(format!("/Items/{item_id}/ExternalIdInfos"));
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
