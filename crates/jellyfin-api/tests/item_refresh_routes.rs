use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewTrickplayInfo, TrickplayInfoRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Item Refresh Tests\", Device=\"Test\", DeviceId=\"item-refresh-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_item_refresh_routes_";

#[tokio::test]
async fn item_refresh_route_requires_elevation_and_accepts_existing_items() {
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
        exercise_item_refresh_route(&task_database_name).await;
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

async fn exercise_item_refresh_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let trickplay = TrickplayInfoRepository::new(fixture.database.clone());
    let trickplay_info = NewTrickplayInfo {
        width: 320,
        height: 180,
        tile_width: 2,
        tile_height: 2,
        thumbnail_count: 6,
        interval: 1_500,
        bandwidth: 22_000,
    };
    trickplay
        .upsert(fixture.item_id, trickplay_info)
        .await
        .expect("trickplay metadata");
    let existing_directory = fixture.trickplay_resolution_directory(320, 2, 2);
    tokio::fs::create_dir_all(&existing_directory)
        .await
        .unwrap();
    tokio::fs::write(
        existing_directory.join("0.jpg"),
        b"temporarily corrupt tile",
    )
    .await
    .unwrap();
    let discovered_directory = fixture.trickplay_resolution_directory(640, 3, 2);
    tokio::fs::create_dir_all(&discovered_directory)
        .await
        .unwrap();
    for index in 0..2 {
        image::RgbImage::from_pixel(1_920, 720, image::Rgb([20, 40, 60]))
            .save_with_format(
                discovered_directory.join(format!("{index}.jpg")),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
    }

    let route = format!("/Items/{}/Refresh", fixture.item_id);
    assert_eq!(
        fixture.post(&route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post(&route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .post(
                &format!("/Items/{}/Refresh", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .post(
                &format!(
                    "/Items/{}/Refresh?metadataRefreshMode=Bogus",
                    fixture.item_id
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .post(
                &format!(
                    "/Items/{}/Refresh?replaceAllImages=definitely",
                    fixture.item_id
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .post(
                &format!(
                    "/Items/{}/Refresh?metadataRefreshMode=Default&regenerateTrickplay=true",
                    fixture.item_id
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        trickplay
            .get(fixture.item_id, trickplay_info.width)
            .await
            .unwrap()
            .is_some(),
        "regeneration only replaces trickplay during a full metadata refresh"
    );
    assert!(fixture.trickplay_item_directory().is_dir());
    let inferred = trickplay
        .get(fixture.item_id, 640)
        .await
        .unwrap()
        .expect("valid managed tiles must be discovered");
    assert_eq!(inferred.height, 360);
    assert_eq!(inferred.tile_width, 3);
    assert_eq!(inferred.tile_height, 2);
    assert_eq!(inferred.thumbnail_count, 12);
    assert_eq!(inferred.interval, 10_000);
    assert!(inferred.bandwidth > 0);

    assert_eq!(
        fixture
            .post(
                &format!(
                    "/Items/{}/Refresh?metadataRefreshMode=FullRefresh&imageRefreshMode=ValidationOnly&replaceAllMetadata=true&replaceAllImages=true&regenerateTrickplay=true",
                    fixture.item_id
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        trickplay
            .get(fixture.item_id, trickplay_info.width)
            .await
            .unwrap(),
        None
    );
    assert!(!fixture.trickplay_item_directory().exists());
    assert_eq!(
        fixture
            .post(
                &format!(
                    "/Items/{}/Refresh?api_key={}",
                    fixture.item_id, fixture.api_key
                ),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    item_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key: String,
    program_data: std::path::PathBuf,
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
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("item-refresh-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("item-refresh-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token =
            session(&devices, admin.id, &format!("item-refresh-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("item-refresh-user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("item-refresh-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let item = BaseItemRepository::new(database.clone())
            .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
            .await
            .expect("item creation");
        let storage_root = std::env::temp_dir().join(format!("jellyfin-item-refresh-{suffix}"));
        let program_data = storage_root.join("programdata");
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Item Refresh Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                &program_data,
                storage_root.join("web"),
                storage_root.join("images"),
                storage_root.join("cache"),
                storage_root.join("metadata"),
            ),
        );

        Self {
            database,
            app,
            item_id: item.id,
            admin_token,
            user_token,
            api_key,
            program_data,
        }
    }

    fn trickplay_item_directory(&self) -> std::path::PathBuf {
        let id = self.item_id.hyphenated().to_string();
        self.program_data.join("trickplay").join(&id[..2]).join(id)
    }

    fn trickplay_resolution_directory(
        &self,
        width: i32,
        tile_width: i32,
        tile_height: i32,
    ) -> std::path::PathBuf {
        self.trickplay_item_directory()
            .join(format!("{width} - {tile_width}x{tile_height}"))
    }

    async fn post(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::post(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        let storage_root = self.program_data.parent().unwrap().to_path_buf();
        self.database.close().await.unwrap();
        match tokio::fs::remove_dir_all(storage_root).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("temporary storage cleanup failed: {error}"),
        }
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Item Refresh Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
