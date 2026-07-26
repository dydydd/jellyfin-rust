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

const DATABASE_PREFIX: &str = "jellyfin_favorite_routes_";
const AUTHORIZATION: &str = "MediaBrowser Client=\"Favorite Tests\", Device=\"PostgreSQL\", DeviceId=\"favorite\", Version=\"1.0\"";

#[tokio::test]
async fn favorite_routes_match_official_contract_with_atomic_postgres_updates() {
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
        exercise_favorite_routes(&task_database_name).await;
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

async fn exercise_favorite_routes(database_name: &str) {
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
    let fixture = FavoriteFixture::new(database.clone()).await;

    assert_route_aliases_and_body(&fixture).await;
    assert_authentication_and_target_user_rules(&fixture).await;
    assert_root_missing_and_visibility(&fixture).await;
    assert_atomic_field_updates_and_idempotency(&fixture).await;

    drop(fixture);
    database.close().await.expect("database pool must close");
}

struct FavoriteFixture {
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

impl FavoriteFixture {
    async fn new(database: DatabaseConnection) -> Self {
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator("favorite-admin")
            .await
            .expect("administrator creation");
        let user = users.create("favorite-user").await.expect("user creation");
        let blocked_user = users
            .create("favorite-blocked")
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
        allowed.presentation_unique_key = Some("movie-presentation-key".to_owned());
        let allowed_item = items.create(allowed).await.expect("allowed movie creation");
        let retired_item = create_item(
            &items,
            "Movie",
            Some(allowed_folder.id),
            "Retired-key movie",
        )
        .await;
        let hidden_item =
            create_item(&items, "Movie", Some(hidden_folder.id), "Hidden movie").await;

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
            .create("favorite-api-key")
            .await
            .expect("API key creation")
            .access_token;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Favorite Test Server".to_owned(),
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

async fn assert_route_aliases_and_body(fixture: &FavoriteFixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let mut seed = NewUserData::new(
        fixture.allowed_item_id,
        fixture.user_id,
        "movie-presentation-key",
    );
    seed.rating = Some(8.5);
    seed.likes = Some(true);
    seed.played = true;
    seed.play_count = 4;
    seed.playback_position_ticks = 123_456;
    seed.audio_stream_index = Some(2);
    seed.subtitle_stream_index = Some(3);
    repository.upsert(seed).await.expect("user-data seed");
    let mut retired_seed = NewUserData::new(
        fixture.allowed_item_id,
        fixture.user_id,
        "older-retired-key",
    );
    retired_seed.rating = Some(2.0);
    repository
        .upsert(retired_seed)
        .await
        .expect("retired user-data seed");

    let modern = modern_route(fixture.allowed_item_id);
    let response = request(&fixture.app, "POST", &modern, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .expect("content type")
            .starts_with("application/json")
    );
    let body = body_json(response).await;
    assert_eq!(body["ItemId"], fixture.allowed_item_id.simple().to_string());
    assert_eq!(body["Key"], "movie-presentation-key");
    assert_eq!(body["IsFavorite"], true);
    assert_eq!(body["Rating"], 8.5);
    assert_eq!(body["Likes"], true);
    assert_eq!(body["Played"], true);
    assert_eq!(body["PlayCount"], 4);
    assert_eq!(body["PlaybackPositionTicks"], 123_456);
    assert!(body.get("PlayedPercentage").is_none());
    assert!(body.get("UnplayedItemCount").is_none());

    let response = request(&fixture.app, "DELETE", &modern, &fixture.user_token).await;
    assert_eq!(body_json(response).await["IsFavorite"], false);

    let legacy = legacy_route(fixture.user_id, fixture.allowed_item_id);
    let response = request(&fixture.app, "POST", &legacy, &fixture.user_token).await;
    assert_eq!(body_json(response).await["IsFavorite"], true);
    let response = request(&fixture.app, "DELETE", &legacy, &fixture.user_token).await;
    assert_eq!(body_json(response).await["IsFavorite"], false);

    let persisted = repository
        .get(
            fixture.allowed_item_id,
            fixture.user_id,
            "movie-presentation-key",
        )
        .await
        .expect("user-data lookup")
        .expect("user-data row");
    assert_eq!(persisted.rating, Some(8.5));
    assert_eq!(persisted.likes, Some(true));
    assert_eq!(persisted.audio_stream_index, Some(2));
    assert_eq!(persisted.subtitle_stream_index, Some(3));
    assert_eq!(persisted.play_count, 4);
    assert!(!persisted.is_favorite);
    let retired_persisted = repository
        .get(
            fixture.allowed_item_id,
            fixture.user_id,
            "older-retired-key",
        )
        .await
        .expect("retired user-data lookup")
        .expect("retired user-data row");
    assert_eq!(retired_persisted.rating, Some(2.0));
    assert!(!retired_persisted.is_favorite);

    assert_retired_key_fallback(fixture, &repository).await;
}

async fn assert_retired_key_fallback(fixture: &FavoriteFixture, repository: &UserDataRepository) {
    let mut retired = NewUserData::new(
        fixture.retired_item_id,
        fixture.user_id,
        "retired-custom-key",
    );
    retired.rating = Some(6.0);
    repository.upsert(retired).await.expect("retired-key seed");
    let response = request(
        &fixture.app,
        "POST",
        &modern_route(fixture.retired_item_id),
        &fixture.user_token,
    )
    .await;
    let body = body_json(response).await;
    assert_eq!(body["Key"], "retired-custom-key");
    assert_eq!(body["Rating"], 6.0);
    let rows = repository
        .get_for_item(fixture.retired_item_id, fixture.user_id)
        .await
        .expect("retired rows lookup");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].custom_data_key, "retired-custom-key");
    assert!(rows[0].is_favorite);
}

async fn assert_authentication_and_target_user_rules(fixture: &FavoriteFixture) {
    assert_anonymous_and_ordinary_target_rules(fixture).await;
    assert_api_key_and_nil_target_rules(fixture).await;
    assert_parental_and_administrator_rules(fixture).await;
}

async fn assert_anonymous_and_ordinary_target_rules(fixture: &FavoriteFixture) {
    let modern = modern_route(fixture.allowed_item_id);
    let legacy = legacy_route(fixture.user_id, fixture.allowed_item_id);
    for (method, route) in [("POST", &modern), ("DELETE", &legacy)] {
        assert_eq!(
            request_without_header(&fixture.app, method, route)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    let other = format!("{modern}?userId={}", fixture.administrator_id);
    assert_eq!(
        request(&fixture.app, "POST", &other, &fixture.user_token)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let nonexistent_other = format!("{modern}?userId={}", Uuid::new_v4());
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &nonexistent_other,
            &fixture.user_token
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

async fn assert_api_key_and_nil_target_rules(fixture: &FavoriteFixture) {
    let modern = modern_route(fixture.allowed_item_id);
    let administrator_target = format!("{modern}?userId={}", fixture.user_id);
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
    let api_key_target = format!(
        "{modern}?userId={}&api_key={}",
        fixture.user_id, fixture.api_key_token
    );
    assert_eq!(
        request_without_header(&fixture.app, "DELETE", &api_key_target)
            .await
            .status(),
        StatusCode::OK
    );

    let api_key_missing_target = format!("{modern}?api_key={}", fixture.api_key_token);
    assert_eq!(
        request_without_header(&fixture.app, "POST", &api_key_missing_target)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let api_key_nil_target = format!(
        "{modern}?userId={}&api_key={}",
        Uuid::nil(),
        fixture.api_key_token
    );
    assert_eq!(
        request_without_header(&fixture.app, "DELETE", &api_key_nil_target)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let legacy_nil = legacy_route(Uuid::nil(), fixture.allowed_item_id);
    assert_eq!(
        request(&fixture.app, "POST", &legacy_nil, &fixture.user_token)
            .await
            .status(),
        StatusCode::OK
    );
    let legacy_nil_api_key = format!("{legacy_nil}?api_key={}", fixture.api_key_token);
    assert_eq!(
        request_without_header(&fixture.app, "POST", &legacy_nil_api_key)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_parental_and_administrator_rules(fixture: &FavoriteFixture) {
    let modern = modern_route(fixture.allowed_item_id);
    let blocked = format!("{modern}?userId={}", fixture.blocked_user_id);
    assert_eq!(
        request(&fixture.app, "POST", &blocked, &fixture.blocked_user_token)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let administrator_target = format!("{modern}?userId={}", fixture.user_id);
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
    let api_key_target = format!(
        "{modern}?userId={}&api_key={}",
        fixture.user_id, fixture.api_key_token
    );
    assert_eq!(
        request_without_header(&fixture.app, "POST", &api_key_target)
            .await
            .status(),
        StatusCode::OK
    );

    let missing_user = format!("{modern}?userId={}", Uuid::new_v4());
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

async fn assert_root_missing_and_visibility(fixture: &FavoriteFixture) {
    assert_eq!(fixture.root_id, USER_ROOT_FOLDER_ID);
    let root_route = format!("{}?userId={}", modern_route(Uuid::nil()), fixture.user_id);
    let response = request(&fixture.app, "POST", &root_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let root = body_json(response).await;
    assert_eq!(root["ItemId"], fixture.root_id.simple().to_string());
    assert_eq!(root["Key"], fixture.root_id.simple().to_string());
    assert_eq!(root["IsFavorite"], true);
    assert!(root.get("Rating").is_none());
    assert!(root.get("Likes").is_none());
    assert!(root.get("LastPlayedDate").is_none());

    let missing = modern_route(Uuid::new_v4());
    for method in ["POST", "DELETE"] {
        assert_eq!(
            request(&fixture.app, method, &missing, &fixture.user_token)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &modern_route(fixture.hidden_item_id),
            &fixture.user_token
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let allowed = request(
        &fixture.app,
        "POST",
        &modern_route(fixture.allowed_item_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(allowed.status(), StatusCode::OK);
}

async fn assert_atomic_field_updates_and_idempotency(fixture: &FavoriteFixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let item_id = fixture.allowed_item_id;
    let key = "movie-presentation-key".to_owned();
    let keys = vec![key.clone(), item_id.simple().to_string()];
    let mut seed = NewUserData::new(item_id, fixture.user_id, &key);
    seed.rating = Some(7.25);
    seed.likes = Some(true);
    seed.playback_position_ticks = 987_654;
    seed.audio_stream_index = Some(4);
    seed.subtitle_stream_index = Some(5);
    repository.upsert(seed).await.expect("concurrency seed");

    let favorite_route = modern_route(item_id);
    let route_update = request(&fixture.app, "POST", &favorite_route, &fixture.user_token);
    let playstate_update = repository.mark_played(item_id, fixture.user_id, &key, None);
    let (response, playstate) = tokio::join!(route_update, playstate_update);
    assert_eq!(response.status(), StatusCode::OK);
    playstate.expect("concurrent playstate update");

    let (first, second, third, fourth) = tokio::join!(
        repository.set_favorite(item_id, fixture.user_id, &keys, true),
        repository.set_favorite(item_id, fixture.user_id, &keys, true),
        repository.set_favorite(item_id, fixture.user_id, &keys, true),
        repository.set_favorite(item_id, fixture.user_id, &keys, true),
    );
    for result in [first, second, third, fourth] {
        result.expect("idempotent favorite upsert");
    }

    let persisted = repository
        .get(item_id, fixture.user_id, &key)
        .await
        .expect("concurrent row lookup")
        .expect("concurrent row");
    assert!(persisted.is_favorite);
    assert!(persisted.played);
    assert_eq!(persisted.play_count, 1);
    assert_eq!(persisted.playback_position_ticks, 0);
    assert_eq!(persisted.rating, Some(7.25));
    assert_eq!(persisted.likes, Some(true));
    assert_eq!(persisted.audio_stream_index, Some(4));
    assert_eq!(persisted.subtitle_stream_index, Some(5));

    let new_item = BaseItemRepository::new(fixture.database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("standalone item creation");
    let inserted = repository
        .set_favorite(
            new_item.id,
            fixture.user_id,
            &[new_item.id.to_string()],
            true,
        )
        .await
        .expect("favorite insert");
    assert!(inserted.is_favorite);
    assert_eq!(inserted.rating, None);
    assert_eq!(inserted.playback_position_ticks, 0);
    assert_eq!(inserted.play_count, 0);
    assert!(!inserted.played);
    assert_eq!(inserted.audio_stream_index, None);
    assert_eq!(inserted.subtitle_stream_index, None);
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
            "Favorite Tests",
            "1.0",
            "PostgreSQL",
            format!("favorite-{label}"),
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
    format!("/UserFavoriteItems/{item_id}")
}

fn legacy_route(user_id: Uuid, item_id: Uuid) -> String {
    format!("/Users/{user_id}/FavoriteItems/{item_id}")
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
