use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
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
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Item Refresh Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            item_id: item.id,
            admin_token,
            user_token,
            api_key,
        }
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
        self.database.close().await.unwrap();
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
