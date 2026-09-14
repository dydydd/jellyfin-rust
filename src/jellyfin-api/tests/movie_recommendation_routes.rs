use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice, NewPerson, NewUserData, PersonRepository, UserDataRepository, entities::item_value,
};
use jellyfin_model::UserPolicy;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Movie Recommendation Tests\", Device=\"Test\", DeviceId=\"movie-recommendation-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_movie_recommendation_routes_";

#[tokio::test]
async fn movie_recommendations_route_matches_weighted_official_categories() {
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
                "/Movies/Recommendations?itemLimit=1&categoryLimit=6&fields=MediaSources",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let recommendations = body.as_array().expect("recommendations");
    assert_eq!(recommendations.len(), 5);
    let recent_category = category(recommendations, "SimilarToRecentlyPlayed", "B Recent Movie");
    assert_eq!(
        recent_category["CategoryId"],
        fixture.recent_movie_id.simple().to_string()
    );
    let recent_item = &recent_category["Items"][0];
    assert_eq!(
        recent_item["Id"],
        fixture.director_movie_id.simple().to_string()
    );
    assert_ne!(
        recent_item["Id"],
        fixture.recent_movie_id.simple().to_string()
    );
    assert_eq!(recent_item["Type"], "Movie");
    assert!(recent_item["MediaSources"].is_array());

    assert_eq!(
        category(recommendations, "SimilarToRecentlyPlayed", "A Older Movie")["Items"][0]["Id"],
        fixture.older_similar_id.simple().to_string()
    );
    assert_eq!(
        category(recommendations, "SimilarToLikedItem", "D Liked Movie")["Items"][0]["Id"],
        fixture.liked_similar_id.simple().to_string()
    );
    let director_category = category(
        recommendations,
        "HasDirectorFromRecentlyPlayed",
        "Alice Director",
    );
    assert_eq!(
        director_category["CategoryId"],
        "b9d62d2a18a3ec626f00cdd59e3aa313"
    );
    assert_eq!(
        director_category["Items"][0]["Id"],
        fixture.director_movie_id.simple().to_string()
    );
    assert_eq!(
        category(recommendations, "HasActorFromRecentlyPlayed", "Bob Actor")["Items"][0]["Id"],
        fixture.actor_movie_id.simple().to_string()
    );

    let api_body = body_json(
        fixture
            .get(
                "/api/Movies/Recommendations?itemLimit=1&categoryLimit=6",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let api_recommendations = api_body.as_array().expect("/api recommendations");
    assert_eq!(
        category(
            api_recommendations,
            "SimilarToRecentlyPlayed",
            "B Recent Movie"
        )["CategoryId"],
        fixture.recent_movie_id.simple().to_string(),
        "the Emby-only numeric-id adaptation must not alter /api"
    );

    let limited = body_json(
        fixture
            .get(
                "/movies/recommendations?itemlimit=1&categorylimit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(limited.as_array().expect("recommendations").len(), 1);
    assert_eq!(limited[0]["BaselineItemName"], "B Recent Movie");
    assert_eq!(
        fixture
            .get(
                "/Movies/Recommendations?categoryLimit=2147483648",
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let count_only = body_json(
        fixture
            .get(
                "/Movies/Recommendations?itemLimit=1&fields=MediaSourceCount",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let count_only_item = &count_only[0]["Items"][0];
    assert_eq!(
        count_only_item["Id"],
        fixture.director_movie_id.simple().to_string()
    );
    assert!(count_only_item.get("MediaSources").is_none());
    assert!(count_only_item.get("MediaSourceCount").is_none());

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_token: String,
    recent_movie_id: Uuid,
    older_similar_id: Uuid,
    liked_similar_id: Uuid,
    director_movie_id: Uuid,
    actor_movie_id: Uuid,
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
        let older_similar = create_item(&items, "Movie", "A Similar Movie", root.id).await;
        let recent_similar = create_item(&items, "Movie", "B Similar Movie", root.id).await;
        let liked_movie = create_item(&items, "Movie", "D Liked Movie", root.id).await;
        let liked_similar = create_item(&items, "Movie", "D Similar Movie", root.id).await;
        let director_movie = create_item(&items, "Movie", "E Director Movie", root.id).await;
        let actor_movie = create_item(&items, "Movie", "F Actor Movie", root.id).await;
        let mut hidden_alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
        hidden_alternate.name = recent_movie.name.clone();
        hidden_alternate.sort_name = recent_movie.sort_name.clone();
        hidden_alternate.parent_id = Some(root.id);
        hidden_alternate.media_type = Some("Video".to_owned());
        hidden_alternate.path =
            Some("/media/movie-recommendations/B Recent Movie - private.mkv".to_owned());
        hidden_alternate.primary_version_id = Some(recent_movie.id);
        let hidden_alternate = items
            .create(hidden_alternate)
            .await
            .expect("hidden alternate movie");
        let item_values = ItemValueRepository::new(database.clone());
        item_values
            .link(
                hidden_alternate.id,
                item_value::ItemValueType::Tags,
                "PrivateVersion",
            )
            .await
            .expect("hidden alternate tag");
        for (item_id, genre) in [
            (older_movie.id, "OlderGenre"),
            (older_similar.id, "OlderGenre"),
            (recent_movie.id, "RecentGenre"),
            (recent_similar.id, "RecentGenre"),
            (hidden_alternate.id, "RecentGenre"),
            (liked_movie.id, "LikedGenre"),
            (liked_similar.id, "LikedGenre"),
        ] {
            item_values
                .link(item_id, item_value::ItemValueType::Genre, genre)
                .await
                .expect("recommendation genre");
        }
        let people = PersonRepository::new(database.clone());
        people
            .link(
                recent_movie.id,
                NewPerson::new("Alice Director"),
                "Director",
                None,
                Some(0),
                0,
            )
            .await
            .expect("recent director");
        people
            .link(
                director_movie.id,
                NewPerson::new("Alice Director"),
                "Director",
                None,
                Some(0),
                0,
            )
            .await
            .expect("matching director");
        people
            .link(
                hidden_alternate.id,
                NewPerson::new("Alice Director"),
                "Director",
                None,
                Some(0),
                0,
            )
            .await
            .expect("blocked matching director");
        people
            .link(
                recent_movie.id,
                NewPerson::new("Bob Actor"),
                "Actor",
                None,
                Some(1),
                1,
            )
            .await
            .expect("recent actor");
        // Official actor recommendations intentionally do not constrain the
        // candidate credit type after selecting Actor/GuestStar seed names.
        people
            .link(
                actor_movie.id,
                NewPerson::new("Bob Actor"),
                "Writer",
                None,
                Some(0),
                0,
            )
            .await
            .expect("matching actor name through another credit type");
        let mut policy = UserPolicy {
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            ..UserPolicy::default()
        };
        policy.blocked_tags = vec!["privateversion".to_owned()];
        users
            .update_policy(user.id, &policy)
            .await
            .expect("restricted recommendation policy");
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
        let mut liked_data = NewUserData::new(liked_movie.id, user.id, liked_movie.id.to_string());
        liked_data.is_favorite = true;
        user_data
            .upsert(liked_data)
            .await
            .expect("favorite user data");

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
            older_similar_id: older_similar.id,
            liked_similar_id: liked_similar.id,
            director_movie_id: director_movie.id,
            actor_movie_id: actor_movie.id,
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
    let status = response.status();
    let body = body_bytes(response).await;
    assert_eq!(status, StatusCode::OK, "response body: {body:?}");
    serde_json::from_slice(&body).expect("JSON response")
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
}

fn category<'a>(recommendations: &'a [Value], kind: &str, baseline: &str) -> &'a Value {
    recommendations
        .iter()
        .find(|category| {
            category["RecommendationType"] == kind && category["BaselineItemName"] == baseline
        })
        .unwrap_or_else(|| panic!("missing {kind} category for {baseline}"))
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
