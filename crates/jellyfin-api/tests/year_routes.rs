use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str =
    "MediaBrowser Client=\"Year Tests\", DeviceId=\"year-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_year_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn year_route_matches_official_authenticated_item_by_name_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/Years/2024", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let virtual_year = body_json(
        fixture
            .request(
                Method::GET,
                "/Years/2024",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(virtual_year["Name"], "2024");
    assert_eq!(virtual_year["Type"], "Year");
    assert_eq!(virtual_year["IsFolder"], true);
    assert_eq!(virtual_year["PresentationUniqueKey"], "Year-2024");
    assert_eq!(
        virtual_year["Id"].as_str().expect("virtual year id").len(),
        32
    );
    assert!(virtual_year.get("name").is_none());

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years/1901",
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years/0",
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Years/2024?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let admin_targeted = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Years/2024?userId={}", fixture.user_id),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_targeted["Name"], "2024");

    let persisted_year = body_json(
        fixture
            .request(
                Method::GET,
                "/Years/1984",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(persisted_year["Name"], "1984");
    assert_eq!(persisted_year["Type"], "Year");
    assert_eq!(
        persisted_year["Id"],
        fixture.persisted_year_id.simple().to_string()
    );

    fixture.cleanup().await;
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

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    user_token: String,
    admin_token: String,
    persisted_year_id: Uuid,
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
            .create_initial_administrator(&format!("year-admin-{suffix}"))
            .await
            .unwrap();
        let user = users.create(&format!("year-user-{suffix}")).await.unwrap();
        let other_user = users
            .create(&format!("year-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        movie.name = Some("The Future".to_owned());
        movie.sort_name = Some("Future".to_owned());
        movie.media_type = Some("Video".to_owned());
        movie.production_year = Some(2024);
        items.create(movie).await.expect("movie creation");

        let persisted_year_id = Uuid::new_v4();
        let mut year = NewBaseItem::new(persisted_year_id, "Year");
        year.name = Some("1984".to_owned());
        year.sort_name = Some("1984".to_owned());
        year.is_folder = true;
        items.create(year).await.expect("year item creation");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Year Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            user_token,
            admin_token,
            persisted_year_id,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Credential::Device(token) = credential {
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

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Year Tests",
            "1.0",
            "Test",
            format!("year-tests-{suffix}"),
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
