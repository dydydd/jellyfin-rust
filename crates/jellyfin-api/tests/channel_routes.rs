use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Channel Tests\", Device=\"Test\", DeviceId=\"channel-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_channel_routes_";

#[tokio::test]
async fn channels_route_lists_persisted_channels_from_postgres() {
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
        exercise_channels_route(&task_database_name).await;
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

async fn exercise_channels_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture.get("/Channels", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels?userId={}", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let body = body_json(
        fixture
            .get("/Channels?startIndex=1&limit=1", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(body["TotalRecordCount"], 2);
    assert_eq!(body["StartIndex"], 1);
    let items = body["Items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["Id"],
        fixture.second_channel_id.simple().to_string()
    );
    assert_eq!(items[0]["Name"], "B Channel");
    assert_eq!(items[0]["Type"], "Channel");

    let filtered = body_json(
        fixture
            .get(
                "/Channels?supportsLatestItems=true&supportsMediaDeletion=false&isFavorite=false",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(filtered["TotalRecordCount"], 2);
    assert!(
        filtered["Items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["Type"] == "Channel")
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_token: String,
    second_channel_id: Uuid,
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
            .create_initial_administrator(&format!("channel-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("channel-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("channel-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("channel-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        create_item(&items, "Channel", "A Channel", root.id).await;
        let second_channel = create_item(&items, "Channel", "B Channel", root.id).await;
        create_item(&items, "Movie", "Ignored Movie", root.id).await;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Channel Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_token,
            second_channel_id: second_channel.id,
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
        self.database.close().await.unwrap();
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    repository.create(item).await.expect("item creation")
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Channel Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).expect("JSON response")
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
