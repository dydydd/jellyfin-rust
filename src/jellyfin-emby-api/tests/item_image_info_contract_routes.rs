use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, NewBaseItem, NewBaseItemImage, NewDevice,
};
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Item Image Info Tests\", DeviceId=\"emby-item-image-info\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_item_image_info_";

#[tokio::test]
async fn item_image_info_filters_jellyfin_only_profile_on_emby_routes() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task was cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let user = UserService::new(database.clone())
        .create_initial_administrator("emby-item-image-info-user")
        .await
        .expect("user creation");
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            user.id,
            "Emby Item Image Info Tests",
            "1.0",
            "Test",
            "emby-item-image-info",
        ))
        .await
        .expect("device session")
        .access_token;

    let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
    item.name = Some("Emby item image info enum fixture".to_owned());
    let item = BaseItemRepository::new(database.clone())
        .create(item)
        .await
        .expect("item creation");
    BaseItemImageRepository::new(database.clone())
        .replace(
            item.id,
            &[
                image(BaseItemImageType::Primary, "/missing/primary.jpg"),
                image(BaseItemImageType::Profile, "/missing/profile.jpg"),
            ],
        )
        .await
        .expect("image metadata");

    let state = AppState::new(
        database.clone(),
        "Emby Item Image Info Test Server".to_owned(),
        "http://127.0.0.1:18096".to_owned(),
    );
    let app = jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state));

    for route in [
        format!("/emby/Items/{}/Images", item.id),
        format!("/emby/iTeMs/{}/iMaGeS", item.id),
    ] {
        let response = request(&app, &route, &token).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(image_types(response).await, ["Primary"], "{route}");
    }

    for route in [
        format!("/Items/{}/Images", item.id),
        format!("/api/Items/{}/Images", item.id),
    ] {
        let response = request(&app, &route, &token).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(
            image_types(response).await,
            ["Primary", "Profile"],
            "{route}"
        );
    }

    database.close().await.expect("database cleanup");
}

fn image(image_type: BaseItemImageType, path: &str) -> NewBaseItemImage {
    NewBaseItemImage {
        image_type,
        image_index: 0,
        path: path.to_owned(),
        date_modified: Utc::now(),
        width: None,
        height: None,
        blurhash: None,
    }
}

async fn request(app: &Router, uri: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn image_types(response: axum::response::Response) -> Vec<String> {
    let value: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("image info JSON");
    value
        .as_array()
        .expect("image info array")
        .iter()
        .map(|image| image["ImageType"].as_str().expect("image type").to_owned())
        .collect()
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
