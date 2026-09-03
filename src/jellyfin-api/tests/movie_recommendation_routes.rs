use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice, NewUserData,
    UserDataRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Movie Recommendation Tests\", Device=\"Test\", DeviceId=\"movie-recommendation-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_movie_recommendation_routes_";

#[tokio::test]
async fn movie_recommendations_route_returns_recent_movie_category_from_postgres() {
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
        exercise_movie_recommendations_route(&task_database_name).await;
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

async fn exercise_movie_recommendations_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture.get("/Movies/Recommendations", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Movies/Recommendations?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Movies/Recommendations?userId={}", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let empty = body_json(
        fixture
            .get(
                "/Movies/Recommendations?categoryLimit=0",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(empty.as_array().expect("recommendations").is_empty());

    let body = body_json(
        fixture
            .get(
                "/Movies/Recommendations?itemLimit=2&fields=MediaSources",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let recommendations = body.as_array().expect("recommendations");
    assert_eq!(recommendations.len(), 1);
    let category = &recommendations[0];
    assert_eq!(category["RecommendationType"], "SimilarToRecentlyPlayed");
    assert_eq!(category["BaselineItemName"], "B Recent Movie");
    assert_eq!(
        category["CategoryId"],
        fixture.recent_movie_id.simple().to_string()
    );
    let items = category["Items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["Id"], fixture.recent_movie_id.simple().to_string());
    assert_eq!(items[0]["Name"], "B Recent Movie");
    assert_eq!(items[0]["Type"], "Movie");
    assert!(items[0]["MediaSources"].is_array());
    assert_eq!(items[1]["Id"], fixture.older_movie_id.simple().to_string());
    assert_eq!(items[1]["Name"], "A Older Movie");
    assert_eq!(items[1]["Type"], "Movie");

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_token: String,
    recent_movie_id: Uuid,
    older_movie_id: Uuid,
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
            .create_initial_administrator(&format!("movie-rec-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("movie-rec-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("movie-rec-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("movie-rec-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let older_movie = create_item(&items, "Movie", "A Older Movie", root.id).await;
        let recent_movie = create_item(&items, "Movie", "B Recent Movie", root.id).await;
        create_item(&items, "Episode", "C Ignored Episode", root.id).await;

        let user_data = UserDataRepository::new(database.clone());
        upsert_played(
            &user_data,
            user.id,
            older_movie.id,
            Utc::now() - Duration::days(3),
        )
        .await;
        upsert_played(&user_data, user.id, recent_movie.id, Utc::now()).await;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Movie Recommendation Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_token,
            recent_movie_id: recent_movie.id,
            older_movie_id: older_movie.id,
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
    item.media_type = Some("Video".to_owned());
    item.path = Some(format!("/media/movie-recommendations/{name}.mkv"));
    repository.create(item).await.expect("item creation")
}

async fn upsert_played(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    last_played_date: chrono::DateTime<Utc>,
) {
    let mut data = NewUserData::new(item_id, user_id, item_id.to_string());
    data.last_played_date = Some(last_played_date);
    data.played = true;
    repository.upsert(data).await.expect("user data");
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Movie Recommendation Tests",
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
