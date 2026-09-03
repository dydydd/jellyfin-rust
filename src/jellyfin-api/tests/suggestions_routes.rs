#![allow(clippy::too_many_lines)]
use std::collections::BTreeSet;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Suggestions Tests\", DeviceId=\"suggestions-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_suggestions_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn suggestions_routes_match_official_auth_filters_and_count_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.get("/Items/Suggestions", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Items/Suggestions?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let suggestions = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video&type=Movie&enableTotalRecordCount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(suggestions["StartIndex"], 0);
    assert_eq!(suggestions["TotalRecordCount"], 2);
    assert_eq!(
        item_names(&suggestions),
        BTreeSet::from([
            format!("Nested Movie {}", fixture.suffix),
            format!("Root Movie {}", fixture.suffix),
        ])
    );
    assert!(
        suggestions["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Movie" && item["MediaType"] == "Video")
    );

    let limited = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video&type=Movie&limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(limited["Items"].as_array().unwrap().len(), 1);
    assert_eq!(
        limited["TotalRecordCount"], 1,
        "official QueryResult uses the returned item count when total counts are disabled"
    );

    let legacy = body_json(
        fixture
            .get(
                &format!(
                    "/Users/{}/Suggestions?MediaType=Audio&Type=Audio",
                    fixture.user_id
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(legacy["TotalRecordCount"], 1);
    assert_eq!(
        item_names(&legacy),
        BTreeSet::from([format!("Audio {}", fixture.suffix)])
    );

    assert_eq!(
        fixture
            .get(
                "/Items/Suggestions?enableTotalRecordCount=not-bool",
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    fixture.cleanup().await;
}

fn item_names(response: &Value) -> BTreeSet<String> {
    response["Items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["Name"].as_str().expect("name").to_owned())
        .collect()
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    suffix: String,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert_temporary_database_name(&database_name);
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");
        administrator.close().await.unwrap();

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
            .create_initial_administrator(&format!("suggestions-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("suggestions-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let root_movie = create_item(
            &items,
            "Movie",
            &format!("Root Movie {suffix}"),
            Some(root.id),
            Some("Video"),
            false,
        )
        .await;
        let folder = create_item(
            &items,
            "Folder",
            &format!("Folder {suffix}"),
            Some(root.id),
            None,
            false,
        )
        .await;
        create_item(
            &items,
            "Movie",
            &format!("Nested Movie {suffix}"),
            Some(folder.id),
            Some("Video"),
            false,
        )
        .await;
        create_item(
            &items,
            "Episode",
            &format!("Episode {suffix}"),
            Some(root.id),
            Some("Video"),
            false,
        )
        .await;
        create_item(
            &items,
            "Audio",
            &format!("Audio {suffix}"),
            Some(root.id),
            Some("Audio"),
            false,
        )
        .await;
        create_item(
            &items,
            "Movie",
            &format!("Virtual Movie {suffix}"),
            Some(root.id),
            Some("Video"),
            true,
        )
        .await;
        assert_ne!(root_movie.id, folder.id);

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Suggestions Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            suffix,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
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
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    media_type: Option<&str>,
    is_virtual_item: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.media_type = media_type.map(ToOwned::to_owned);
    item.is_folder = item_type == "Folder";
    item.is_virtual_item = is_virtual_item;
    repository.create(item).await.expect("item creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Suggestions Tests",
            "1.0",
            "Test",
            format!("suggestions-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
