use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    entities::{base_item, user},
};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Remote Image Tests\", Device=\"Test\", DeviceId=\"remote-images\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_remote_image_routes_";

#[tokio::test]
async fn remote_image_routes_match_official_empty_provider_contract() {
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
        exercise_remote_image_routes(&task_database_name).await;
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

async fn exercise_remote_image_routes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let images_route = format!(
        "/Items/{}/RemoteImages?type=Primary&startIndex=10&limit=5&providerName=Example&includeAllLanguages=true",
        fixture.item_id
    );
    let providers_route = format!("/Items/{}/RemoteImages/Providers", fixture.item_id);
    let download_route = format!(
        "/Items/{}/RemoteImages/Download?type=Primary&imageUrl=https%3A%2F%2Fexample.invalid%2Fposter.jpg",
        fixture.item_id
    );

    let unauthenticated = fixture.get_without_auth(&images_route).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let missing = fixture
        .get(
            &format!("/Items/{}/RemoteImages", Uuid::new_v4()),
            &fixture.user_token,
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let images = fixture.get(&images_route, &fixture.user_token).await;
    assert_eq!(images.status(), StatusCode::OK);
    assert_eq!(
        body_json(images).await,
        json!({
            "Images": [],
            "TotalRecordCount": 0,
            "Providers": []
        })
    );

    let providers = fixture.get(&providers_route, &fixture.user_token).await;
    assert_eq!(providers.status(), StatusCode::OK);
    assert_eq!(body_json(providers).await, Value::Array(Vec::new()));

    let missing_download_type = fixture
        .post(
            &format!("/Items/{}/RemoteImages/Download", fixture.item_id),
            &fixture.admin_token,
        )
        .await;
    assert_eq!(missing_download_type.status(), StatusCode::BAD_REQUEST);

    let regular_download = fixture.post(&download_route, &fixture.user_token).await;
    assert_eq!(regular_download.status(), StatusCode::FORBIDDEN);

    let missing_download = fixture
        .post(
            &format!(
                "/Items/{}/RemoteImages/Download?type=Primary",
                Uuid::new_v4()
            ),
            &fixture.admin_token,
        )
        .await;
    assert_eq!(missing_download.status(), StatusCode::NOT_FOUND);

    let admin_download = fixture.post(&download_route, &fixture.admin_token).await;
    assert_eq!(admin_download.status(), StatusCode::NOT_FOUND);

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
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
            .create_initial_administrator(&format!("remote-image-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("remote-image-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = devices
            .create_session(NewDevice::new(
                admin.id,
                "Remote Image Tests",
                "1.0",
                "Test",
                format!("remote-image-admin-{suffix}"),
            ))
            .await
            .expect("admin session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "Remote Image Tests",
                "1.0",
                "Test",
                format!("remote-image-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some("Remote Image Movie".to_owned());
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Remote Image Movie {suffix}.mkv"));
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("movie item creation");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Remote Image Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            item_id: item.id,
        }
    }

    async fn get(&self, uri: &str, token: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{token}\""),
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

    async fn post(&self, uri: &str, token: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::post(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{token}\""),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_many()
            .filter(base_item::Column::Id.eq(self.item_id))
            .exec(&self.database)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        self.database.close().await.unwrap();
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

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
