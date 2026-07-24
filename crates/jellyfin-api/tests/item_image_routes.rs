use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, NewBaseItem, NewBaseItemImage, NewDevice,
    entities::{base_item, user},
};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Item Image Tests\", Device=\"Test\", DeviceId=\"item-images\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_item_image_routes_";
const ITEM_PATH: &str = "/media/image-info-test.mkv";
const IMAGE_TAG: &str = "fdcbd27b24b37e862315a492f0300d8c";

#[tokio::test]
async fn item_image_infos_match_official_postgres_contract() {
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
        exercise_item_image_infos(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_item_image_infos(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = format!("/Items/{}/Images", fixture.item_id);

    let unauthenticated = fixture.get_without_auth(&route).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let missing = fixture
        .get(&format!("/Items/{}/Images", Uuid::new_v4()))
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let empty = fixture
        .get(&format!("/Items/{}/Images", fixture.empty_item_id))
        .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(body_json(empty).await, json!([]));

    let response = fixture.get(&route).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "ImageType": "Primary",
                "ImageIndex": null,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("poster.jpg"),
                "BlurHash": "primary-blurhash",
                "Height": 900,
                "Width": 600,
                "Size": 11
            },
            {
                "ImageType": "Logo",
                "ImageIndex": null,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("missing-logo.png"),
                "BlurHash": null,
                "Height": null,
                "Width": null,
                "Size": 0
            },
            {
                "ImageType": "Backdrop",
                "ImageIndex": 0,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("backdrop.jpg"),
                "BlurHash": "backdrop-blurhash",
                "Height": 1080,
                "Width": 1920,
                "Size": 8
            },
            {
                "ImageType": "Backdrop",
                "ImageIndex": 1,
                "ImageTag": IMAGE_TAG,
                "Path": "https://images.example.invalid/backdrop.jpg",
                "BlurHash": null,
                "Height": null,
                "Width": null,
                "Size": 0
            },
            {
                "ImageType": "Chapter",
                "ImageIndex": 0,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("chapter.jpg"),
                "BlurHash": "chapter-blurhash",
                "Height": 360,
                "Width": 640,
                "Size": 7
            }
        ])
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    temporary: TempDirectory,
    user_id: Uuid,
    item_id: Uuid,
    empty_item_id: Uuid,
    token: String,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
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

        let suffix = Uuid::new_v4().simple().to_string();
        let temporary = TempDirectory::new();
        fs::write(temporary.path().join("poster.jpg"), b"poster-data").unwrap();
        fs::write(temporary.path().join("backdrop.jpg"), b"backdrop").unwrap();
        fs::write(temporary.path().join("chapter.jpg"), b"chapter").unwrap();

        let users = UserService::new(database.clone());
        let user = users
            .create_initial_administrator(&format!("item-image-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Item Image Tests",
                "1.0",
                "Test",
                format!("item-images-{suffix}"),
            ))
            .await
            .expect("administrator session")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some("Image Info Movie".to_owned());
        item.media_type = Some("Video".to_owned());
        item.path = Some(ITEM_PATH.to_owned());
        let item = items.create(item).await.expect("movie item creation");

        let mut empty_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        empty_item.name = Some("No Images Movie".to_owned());
        empty_item.media_type = Some("Video".to_owned());
        empty_item.path = Some("/media/no-images.mkv".to_owned());
        let empty_item = items
            .create(empty_item)
            .await
            .expect("empty movie item creation");

        let modified = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).single().unwrap();
        BaseItemImageRepository::new(database.clone())
            .replace(
                item.id,
                &[
                    image(
                        BaseItemImageType::Primary,
                        0,
                        temporary.path().join("poster.jpg"),
                        modified,
                        Some((600, 900)),
                        Some("primary-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Backdrop,
                        4,
                        temporary.path().join("backdrop.jpg"),
                        modified,
                        Some((1920, 1080)),
                        Some("backdrop-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Backdrop,
                        9,
                        PathBuf::from("https://images.example.invalid/backdrop.jpg"),
                        modified,
                        Some((1280, 720)),
                        Some("remote-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Logo,
                        0,
                        temporary.path().join("missing-logo.png"),
                        modified,
                        Some((400, 200)),
                        Some("missing-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Chapter,
                        8,
                        temporary.path().join("chapter.jpg"),
                        modified,
                        Some((640, 360)),
                        Some("chapter-blurhash"),
                    ),
                ],
            )
            .await
            .expect("image metadata replacement");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Item Image Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            temporary,
            user_id: user.id,
            item_id: item.id,
            empty_item_id: empty_item.id,
            token,
        }
    }

    async fn get(&self, uri: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{}\"", self.token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_without_auth(&self, uri: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn path(&self, name: &str) -> String {
        self.temporary
            .path()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_many()
            .filter(base_item::Column::Id.is_in([self.item_id, self.empty_item_id]))
            .exec(&self.database)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        self.database.close().await.unwrap();
    }
}

fn image(
    image_type: BaseItemImageType,
    image_index: u32,
    path: PathBuf,
    date_modified: chrono::DateTime<Utc>,
    dimensions: Option<(u32, u32)>,
    blurhash: Option<&str>,
) -> NewBaseItemImage {
    NewBaseItemImage {
        image_type,
        image_index,
        path: path.to_string_lossy().into_owned(),
        date_modified,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
        blurhash: blurhash.map(str::to_owned),
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-item-image-api-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
