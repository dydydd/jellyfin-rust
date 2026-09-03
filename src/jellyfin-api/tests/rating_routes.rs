use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewUserData, USER_ROOT_FOLDER_ID, UserDataRepository,
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, UserPolicy};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_rating_routes_";
const AUTHORIZATION: &str = "MediaBrowser Client=\"Rating Tests\", Device=\"PostgreSQL\", DeviceId=\"rating\", Version=\"1.0\"";

#[tokio::test]
async fn rating_routes_match_official_contract_with_atomic_postgres_updates() {
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
        exercise_rating_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_rating_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let fixture = RatingFixture::new(database.clone()).await;

    assert_rating_values_aliases_and_keys(&fixture).await;
    assert_authentication_and_target_user_rules(&fixture).await;
    assert_root_missing_and_visibility(&fixture).await;
    assert_atomic_updates_and_defaults(&fixture).await;

    drop(fixture);
    database.close().await.expect("database pool must close");
}

struct RatingFixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    blocked_user_id: Uuid,
    blocked_user_token: String,
    api_key_token: String,
    root_id: Uuid,
    allowed_item_id: Uuid,
    hidden_item_id: Uuid,
    retired_item_id: Uuid,
}

impl RatingFixture {
    async fn new(database: DatabaseConnection) -> Self {
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator("rating-admin")
            .await
            .expect("administrator creation");
        let user = users.create("rating-user").await.expect("user creation");
        let blocked_user = users
            .create("rating-blocked")
            .await
            .expect("blocked user creation");

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root creation");
        let allowed_folder =
            create_item(&items, "CollectionFolder", Some(root.id), "Allowed library").await;
        let hidden_folder =
            create_item(&items, "CollectionFolder", Some(root.id), "Hidden library").await;
        let mut allowed = NewBaseItem::new(Uuid::new_v4(), "Movie");
        allowed.parent_id = Some(allowed_folder.id);
        allowed.name = Some("Allowed movie".to_owned());
        allowed.presentation_unique_key = Some("rating-presentation-key".to_owned());
        let allowed_item = items.create(allowed).await.expect("allowed movie creation");
        let hidden_item =
            create_item(&items, "Movie", Some(hidden_folder.id), "Hidden movie").await;
        let retired_item = create_item(
            &items,
            "Movie",
            Some(allowed_folder.id),
            "Retired-key movie",
        )
        .await;

        let mut user_policy = policy();
        user_policy.enable_all_folders = false;
        user_policy.enabled_folders = vec![allowed_folder.id];
        users
            .update_policy(user.id, &user_policy)
            .await
            .expect("limited folder policy");
        users
            .update_policy(blocked_user.id, &blocked_schedule_policy(false))
            .await
            .expect("blocked schedule policy");
        users
            .update_policy(administrator.id, &blocked_schedule_policy(true))
            .await
            .expect("administrator blocked schedule policy");

        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session_token(&devices, administrator.id, "admin").await;
        let user_token = session_token(&devices, user.id, "user").await;
        let blocked_user_token = session_token(&devices, blocked_user.id, "blocked").await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create("rating-api-key")
            .await
            .expect("API key creation")
            .access_token;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Rating Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            administrator_id: administrator.id,
            administrator_token,
            user_id: user.id,
            user_token,
            blocked_user_id: blocked_user.id,
            blocked_user_token,
            api_key_token,
            root_id: root.id,
            allowed_item_id: allowed_item.id,
            hidden_item_id: hidden_item.id,
            retired_item_id: retired_item.id,
        }
    }
}

async fn assert_rating_values_aliases_and_keys(fixture: &RatingFixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    seed_current_and_retired_rows(fixture, &repository).await;
    let modern = modern_route(fixture.allowed_item_id);

    let liked = body_json(
        request(
            &fixture.app,
            "POST",
            &format!("{modern}?likes=true"),
            &fixture.user_token,
        )
        .await,
    )
    .await;
    assert_rating_body(&liked, fixture.allowed_item_id, Some(10.0), Some(true));
    assert_eq!(liked["Key"], "rating-presentation-key");
    assert_eq!(liked["IsFavorite"], true);
    assert_eq!(liked["Played"], true);
    assert_eq!(liked["PlayCount"], 4);
    assert_eq!(liked["PlaybackPositionTicks"], 123_456);

    let disliked = body_json(
        request(
            &fixture.app,
            "POST",
            &format!("{modern}?likes=false"),
            &fixture.user_token,
        )
        .await,
    )
    .await;
    assert_rating_body(&disliked, fixture.allowed_item_id, Some(1.0), Some(false));

    let cleared =
        body_json(request(&fixture.app, "POST", &modern, &fixture.user_token).await).await;
    assert_rating_body(&cleared, fixture.allowed_item_id, None, None);
    let legacy = legacy_route(fixture.user_id, fixture.allowed_item_id);
    let liked = body_json(
        request(
            &fixture.app,
            "POST",
            &format!("{legacy}?likes=true"),
            &fixture.user_token,
        )
        .await,
    )
    .await;
    assert_rating_body(&liked, fixture.allowed_item_id, Some(10.0), Some(true));
    let deleted =
        body_json(request(&fixture.app, "DELETE", &legacy, &fixture.user_token).await).await;
    assert_rating_body(&deleted, fixture.allowed_item_id, None, None);

    let current = repository
        .get(
            fixture.allowed_item_id,
            fixture.user_id,
            "rating-presentation-key",
        )
        .await
        .expect("current lookup")
        .expect("current row");
    assert_eq!(current.rating, None);
    assert_eq!(current.likes, None);
    assert!(current.is_favorite);
    assert_eq!(current.audio_stream_index, Some(2));
    assert_eq!(current.subtitle_stream_index, Some(3));
    let retired = repository
        .get(fixture.allowed_item_id, fixture.user_id, "older-rating-key")
        .await
        .expect("retired lookup")
        .expect("retired row");
    assert_eq!(retired.rating, Some(2.0));
    assert_eq!(retired.likes, Some(false));

    assert_retired_key_fallback(fixture, &repository).await;
}

async fn seed_current_and_retired_rows(fixture: &RatingFixture, repository: &UserDataRepository) {
    let mut current = NewUserData::new(
        fixture.allowed_item_id,
        fixture.user_id,
        "rating-presentation-key",
    );
    current.rating = Some(8.5);
    current.likes = Some(true);
    current.is_favorite = true;
    current.played = true;
    current.play_count = 4;
    current.playback_position_ticks = 123_456;
    current.audio_stream_index = Some(2);
    current.subtitle_stream_index = Some(3);
    repository.upsert(current).await.expect("current seed");
    let mut retired =
        NewUserData::new(fixture.allowed_item_id, fixture.user_id, "older-rating-key");
    retired.rating = Some(2.0);
    retired.likes = Some(false);
    repository.upsert(retired).await.expect("retired seed");
}

async fn assert_retired_key_fallback(fixture: &RatingFixture, repository: &UserDataRepository) {
    let mut retired = NewUserData::new(
        fixture.retired_item_id,
        fixture.user_id,
        "retired-custom-key",
    );
    retired.is_favorite = true;
    repository.upsert(retired).await.expect("retired-only seed");
    let response = request(
        &fixture.app,
        "POST",
        &format!("{}?likes=false", modern_route(fixture.retired_item_id)),
        &fixture.user_token,
    )
    .await;
    let body = body_json(response).await;
    assert_eq!(body["Key"], "retired-custom-key");
    assert_eq!(body["Rating"], 1.0);
    assert_eq!(body["Likes"], false);
    let rows = repository
        .get_for_item(fixture.retired_item_id, fixture.user_id)
        .await
        .expect("retired rows lookup");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].custom_data_key, "retired-custom-key");
    assert!(rows[0].is_favorite);
}

async fn assert_authentication_and_target_user_rules(fixture: &RatingFixture) {
    assert_anonymous_and_ordinary_rules(fixture).await;
    assert_api_key_and_nil_rules(fixture).await;
    assert_parental_and_administrator_rules(fixture).await;
}

async fn assert_anonymous_and_ordinary_rules(fixture: &RatingFixture) {
    let modern = modern_route(fixture.allowed_item_id);
    let legacy = legacy_route(fixture.user_id, fixture.allowed_item_id);
    for (method, route) in [("POST", format!("{modern}?likes=true")), ("DELETE", legacy)] {
        assert_eq!(
            request_without_header(&fixture.app, method, &route)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for target in [fixture.administrator_id, Uuid::new_v4()] {
        let route = format!("{modern}?userId={target}&likes=true");
        assert_eq!(
            request(&fixture.app, "POST", &route, &fixture.user_token)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

async fn assert_api_key_and_nil_rules(fixture: &RatingFixture) {
    let modern = modern_route(fixture.allowed_item_id);
    let administrator_target = format!("{modern}?userId={}&likes=true", fixture.user_id);
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &administrator_target,
            &fixture.administrator_token
        )
        .await
        .status(),
        StatusCode::OK
    );
    let api_target = format!(
        "{modern}?userId={}&likes=false&api_key={}",
        fixture.user_id, fixture.api_key_token
    );
    assert_eq!(
        request_without_header(&fixture.app, "POST", &api_target)
            .await
            .status(),
        StatusCode::OK
    );
    for route in [
        format!("{modern}?likes=true&api_key={}", fixture.api_key_token),
        format!(
            "{modern}?userId={}&api_key={}",
            Uuid::nil(),
            fixture.api_key_token
        ),
    ] {
        assert_eq!(
            request_without_header(&fixture.app, "POST", &route)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let legacy_nil = legacy_route(Uuid::nil(), fixture.allowed_item_id);
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!("{legacy_nil}?likes=true"),
            &fixture.user_token
        )
        .await
        .status(),
        StatusCode::OK
    );
    let legacy_api_nil = format!("{legacy_nil}?likes=true&api_key={}", fixture.api_key_token);
    assert_eq!(
        request_without_header(&fixture.app, "POST", &legacy_api_nil)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_parental_and_administrator_rules(fixture: &RatingFixture) {
    let modern = modern_route(fixture.allowed_item_id);
    let blocked = format!("{modern}?userId={}&likes=true", fixture.blocked_user_id);
    assert_eq!(
        request(&fixture.app, "POST", &blocked, &fixture.blocked_user_token)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let administrator_target = format!("{modern}?userId={}&likes=true", fixture.user_id);
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &administrator_target,
            &fixture.administrator_token
        )
        .await
        .status(),
        StatusCode::OK
    );
    let api_target = format!(
        "{modern}?userId={}&likes=true&api_key={}",
        fixture.user_id, fixture.api_key_token
    );
    assert_eq!(
        request_without_header(&fixture.app, "POST", &api_target)
            .await
            .status(),
        StatusCode::OK
    );
    let missing_user = format!("{modern}?userId={}&likes=true", Uuid::new_v4());
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &missing_user,
            &fixture.administrator_token
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_root_missing_and_visibility(fixture: &RatingFixture) {
    assert_eq!(fixture.root_id, USER_ROOT_FOLDER_ID);
    let root_route = format!(
        "{}?userId={}&likes=true",
        modern_route(Uuid::nil()),
        fixture.user_id
    );
    let response = request(&fixture.app, "POST", &root_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let root = body_json(response).await;
    assert_rating_body(&root, fixture.root_id, Some(10.0), Some(true));
    assert_eq!(root["Key"], fixture.root_id.simple().to_string());
    let root_delete = format!("{}?userId={}", modern_route(Uuid::nil()), fixture.user_id);
    let cleared =
        body_json(request(&fixture.app, "DELETE", &root_delete, &fixture.user_token).await).await;
    assert_rating_body(&cleared, fixture.root_id, None, None);

    let missing = modern_route(Uuid::new_v4());
    for method in ["POST", "DELETE"] {
        let route = if method == "POST" {
            format!("{missing}?likes=true")
        } else {
            missing.clone()
        };
        assert_eq!(
            request(&fixture.app, method, &route, &fixture.user_token)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let hidden = format!("{}?likes=true", modern_route(fixture.hidden_item_id));
    assert_eq!(
        request(&fixture.app, "POST", &hidden, &fixture.user_token)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let allowed = format!("{}?likes=true", modern_route(fixture.allowed_item_id));
    assert_eq!(
        request(&fixture.app, "POST", &allowed, &fixture.user_token)
            .await
            .status(),
        StatusCode::OK
    );
}

async fn assert_atomic_updates_and_defaults(fixture: &RatingFixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let item_id = fixture.allowed_item_id;
    let key = "rating-presentation-key".to_owned();
    let keys = vec![key.clone(), item_id.simple().to_string()];
    let mut seed = NewUserData::new(item_id, fixture.user_id, &key);
    seed.is_favorite = false;
    seed.played = true;
    seed.play_count = 8;
    seed.playback_position_ticks = 456_789;
    seed.audio_stream_index = Some(4);
    seed.subtitle_stream_index = Some(5);
    repository.upsert(seed).await.expect("atomic seed");

    let route = format!("{}?likes=false", modern_route(item_id));
    let rating_update = request(&fixture.app, "POST", &route, &fixture.user_token);
    let favorite_update = repository.set_favorite(item_id, fixture.user_id, &keys, true);
    let (response, favorite) = tokio::join!(rating_update, favorite_update);
    assert_eq!(response.status(), StatusCode::OK);
    favorite.expect("concurrent favorite update");

    let (first, second, third, fourth) = tokio::join!(
        repository.set_rating(item_id, fixture.user_id, &keys, Some(true)),
        repository.set_rating(item_id, fixture.user_id, &keys, Some(true)),
        repository.set_rating(item_id, fixture.user_id, &keys, Some(true)),
        repository.set_rating(item_id, fixture.user_id, &keys, Some(true)),
    );
    for result in [first, second, third, fourth] {
        result.expect("idempotent rating upsert");
    }
    let persisted = repository
        .get(item_id, fixture.user_id, &key)
        .await
        .expect("atomic lookup")
        .expect("atomic row");
    assert_eq!(persisted.rating, Some(10.0));
    assert_eq!(persisted.likes, Some(true));
    assert!(persisted.is_favorite);
    assert!(persisted.played);
    assert_eq!(persisted.play_count, 8);
    assert_eq!(persisted.playback_position_ticks, 456_789);
    assert_eq!(persisted.audio_stream_index, Some(4));
    assert_eq!(persisted.subtitle_stream_index, Some(5));

    let new_item = BaseItemRepository::new(fixture.database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("standalone item creation");
    let inserted = repository
        .set_rating(
            new_item.id,
            fixture.user_id,
            &[new_item.id.to_string()],
            Some(false),
        )
        .await
        .expect("rating insert");
    assert_eq!(inserted.rating, Some(1.0));
    assert_eq!(inserted.likes, Some(false));
    assert!(!inserted.is_favorite);
    assert_eq!(inserted.playback_position_ticks, 0);
    assert_eq!(inserted.play_count, 0);
    assert!(!inserted.played);
}

fn assert_rating_body(body: &Value, item_id: Uuid, rating: Option<f64>, likes: Option<bool>) {
    assert_eq!(body["ItemId"], item_id.simple().to_string());
    match rating {
        Some(value) => assert_eq!(body["Rating"], value),
        None => assert!(body.get("Rating").is_none()),
    }
    match likes {
        Some(value) => assert_eq!(body["Likes"], value),
        None => assert!(body.get("Likes").is_none()),
    }
}

async fn create_item(
    items: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = parent_id;
    item.name = Some(name.to_owned());
    items.create(item).await.expect("base item creation")
}

async fn session_token(devices: &DeviceRepository, user_id: Uuid, label: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Rating Tests",
            "1.0",
            "PostgreSQL",
            format!("rating-{label}"),
        ))
        .await
        .expect("device session creation")
        .access_token
}

fn policy() -> UserPolicy {
    UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

fn blocked_schedule_policy(is_administrator: bool) -> UserPolicy {
    UserPolicy {
        is_administrator,
        access_schedules: vec![AccessSchedule {
            day_of_week: DynamicDayOfWeek::Everyday,
            start_hour: 18.0,
            end_hour: 6.0,
        }],
        ..policy()
    }
}

fn modern_route(item_id: Uuid) -> String {
    format!("/UserItems/{item_id}/Rating")
}

fn legacy_route(user_id: Uuid, item_id: Uuid) -> String {
    format!("/Users/{user_id}/Items/{item_id}/Rating")
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn request_without_header(
    app: &axum::Router,
    method: &str,
    uri: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn body_json(response: axum::response::Response) -> Value {
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .expect("content type")
            .starts_with("application/json")
    );
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
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
