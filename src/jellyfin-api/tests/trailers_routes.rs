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

const AUTHORIZATION: &str = "MediaBrowser Client=\"Trailer Tests\", Device=\"Test\", DeviceId=\"trailer-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_trailers_routes_";

#[tokio::test]
async fn trailers_route_reuses_items_query_with_trailer_type_filter() {
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
        exercise_trailers_route(&task_database_name).await;
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

async fn exercise_trailers_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture.get("/Trailers?recursive=true", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let body = body_json(
        fixture
            .get(
                &format!(
                    "/Trailers?recursive=true&searchTerm={}&sortBy=SortName",
                    fixture.suffix
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(body["TotalRecordCount"], 2);
    assert_eq!(body["StartIndex"], 0);
    assert_eq!(
        body["Items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["Type"].as_str().expect("type"))
            .collect::<Vec<_>>(),
        ["Trailer", "Trailer"]
    );
    assert_eq!(
        body["Items"][0]["Name"],
        format!("A Trailer {}", fixture.suffix)
    );
    assert_eq!(
        body["Items"][1]["Name"],
        format!("B Trailer {}", fixture.suffix)
    );

    let caller_cannot_override_official_type = body_json(
        fixture
            .get(
                &format!(
                    "/Trailers?recursive=true&searchTerm={}&includeItemTypes=Movie",
                    fixture.suffix
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(caller_cannot_override_official_type["TotalRecordCount"], 2);
    assert!(
        caller_cannot_override_official_type["Items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["Type"] == "Trailer")
    );

    let ids_still_intersect_trailers = body_json(
        fixture
            .get(
                &format!(
                    "/Trailers?recursive=true&ids={},{}",
                    fixture.movie_id, fixture.first_trailer_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(ids_still_intersect_trailers["TotalRecordCount"], 1);
    assert_eq!(
        ids_still_intersect_trailers["Items"][0]["Id"],
        fixture.first_trailer_id.simple().to_string()
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    suffix: String,
    user_token: String,
    movie_id: Uuid,
    first_trailer_id: Uuid,
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
        let user = UserService::new(database.clone())
            .create(&format!("trailer-user-{suffix}"))
            .await
            .expect("user creation");
        let user_token = session(
            &DeviceRepository::new(database.clone()),
            user.id,
            &format!("user-{suffix}"),
        )
        .await;
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let first_trailer =
            create_item(&items, "Trailer", &format!("A Trailer {suffix}"), root.id).await;
        create_item(&items, "Trailer", &format!("B Trailer {suffix}"), root.id).await;
        let movie = create_item(&items, "Movie", &format!("C Movie {suffix}"), root.id).await;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Trailer Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            suffix,
            user_token,
            movie_id: movie.id,
            first_trailer_id: first_trailer.id,
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
            "Trailer Tests",
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
