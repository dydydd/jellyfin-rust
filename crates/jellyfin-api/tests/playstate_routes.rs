use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, NewUserData,
    UserDataRepository,
    entities::{api_key, user},
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, UserPolicy};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Playstate Tests\", Device=\"Test\", DeviceId=\"playstate\", Version=\"1.0\"";

#[tokio::test]
async fn delete_mark_unplayed_item_nonexistent_user_id_not_found() {
    let fixture = PlaystateFixture::new().await;
    let response = request(
        &fixture.app,
        "DELETE",
        &playstate_route(Uuid::new_v4(), Uuid::new_v4()),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    fixture.cleanup().await;
}

#[tokio::test]
async fn post_mark_played_item_nonexistent_user_id_not_found() {
    let fixture = PlaystateFixture::new().await;
    let response = request(
        &fixture.app,
        "POST",
        &playstate_route(Uuid::new_v4(), Uuid::new_v4()),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    fixture.cleanup().await;
}

#[tokio::test]
async fn delete_mark_unplayed_item_nonexistent_item_id_not_found() {
    let fixture = PlaystateFixture::new().await;
    let response = request(
        &fixture.app,
        "DELETE",
        &playstate_route(fixture.administrator_id, Uuid::new_v4()),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    fixture.cleanup().await;
}

#[tokio::test]
async fn post_mark_played_item_nonexistent_item_id_not_found() {
    let fixture = PlaystateFixture::new().await;
    let response = request(
        &fixture.app,
        "POST",
        &playstate_route(fixture.administrator_id, Uuid::new_v4()),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    fixture.cleanup().await;
}

#[tokio::test]
async fn played_unplayed_permissions_and_concurrency_use_postgres_state() {
    let fixture = PlaystateFixture::new().await;
    assert_authentication_and_permissions(&fixture).await;
    assert_modern_target_user_semantics(&fixture).await;
    assert_default_parental_policy(&fixture).await;
    assert_modern_not_found_semantics(&fixture).await;
    assert_played_and_unplayed(&fixture).await;
    assert_concurrent_manual_play_is_idempotent(&fixture).await;
    fixture.cleanup().await;
}

struct PlaystateFixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    blocked_user_id: Uuid,
    blocked_user_token: String,
    api_key_id: i64,
    api_key_token: String,
    item_id: Uuid,
}

impl PlaystateFixture {
    async fn new() -> Self {
        let database = test_database().await;
        let users = UserService::new(database.clone());
        let suffix = Uuid::new_v4().simple().to_string();
        let administrator = users
            .create_initial_administrator(&format!("playstate-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("playstate-user-{suffix}"))
            .await
            .expect("user creation");
        let blocked_user = users
            .create(&format!("playstate-blocked-{suffix}"))
            .await
            .expect("blocked user creation");
        users
            .update_policy(blocked_user.id, &blocked_policy())
            .await
            .expect("blocked user policy");
        let devices = DeviceRepository::new(database.clone());
        let administrator_session = devices
            .create_session(NewDevice::new(
                administrator.id,
                "Playstate Tests",
                "1.0",
                "Test",
                format!("admin-{suffix}"),
            ))
            .await
            .expect("administrator session");
        let user_session = devices
            .create_session(NewDevice::new(
                user.id,
                "Playstate Tests",
                "1.0",
                "Test",
                format!("user-{suffix}"),
            ))
            .await
            .expect("user session");
        let blocked_user_session = devices
            .create_session(NewDevice::new(
                blocked_user.id,
                "Playstate Tests",
                "1.0",
                "Test",
                format!("blocked-{suffix}"),
            ))
            .await
            .expect("blocked user session");
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("playstate-key-{suffix}"))
            .await
            .expect("API key creation");
        let items = BaseItemRepository::new(database.clone());
        let item = items
            .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
            .await
            .expect("base item creation");
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Playstate Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            administrator_id: administrator.id,
            administrator_token: administrator_session.access_token,
            user_id: user.id,
            user_token: user_session.access_token,
            blocked_user_id: blocked_user.id,
            blocked_user_token: blocked_user_session.access_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            item_id: item.id,
        }
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API key cleanup");
        BaseItemRepository::new(self.database.clone())
            .delete(self.item_id)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([
                self.administrator_id,
                self.user_id,
                self.blocked_user_id,
            ]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

async fn assert_authentication_and_permissions(fixture: &PlaystateFixture) {
    let route = playstate_route(fixture.user_id, fixture.item_id);
    let response = fixture
        .app
        .clone()
        .oneshot(Request::post(&route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let administrator_route = playstate_route(fixture.administrator_id, fixture.item_id);
    let response = request(
        &fixture.app,
        "POST",
        &administrator_route,
        &fixture.user_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = request(&fixture.app, "POST", &route, &fixture.administrator_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = request(&fixture.app, "DELETE", &route, &fixture.administrator_token).await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn assert_modern_target_user_semantics(fixture: &PlaystateFixture) {
    let self_route = modern_playstate_route(fixture.item_id);
    let response = request_without_header(&fixture.app, "POST", &self_route).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = request(&fixture.app, "POST", &self_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = request(&fixture.app, "DELETE", &self_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);

    let target_route = format!("{self_route}?userId={}", fixture.user_id);
    let response = request(
        &fixture.app,
        "POST",
        &target_route,
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let forbidden_route = format!("{self_route}?userId={}", fixture.administrator_id);
    let response = request(&fixture.app, "POST", &forbidden_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let api_key_route = format!(
        "{self_route}?userId={}&api_key={}",
        fixture.user_id, fixture.api_key_token
    );
    let response = request_without_header(&fixture.app, "POST", &api_key_route).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = request_without_header(&fixture.app, "DELETE", &api_key_route).await;
    assert_eq!(response.status(), StatusCode::OK);

    let legacy_api_key_route = format!(
        "{}?api_key={}",
        playstate_route(fixture.user_id, fixture.item_id),
        fixture.api_key_token
    );
    let response = request_without_header(&fixture.app, "POST", &legacy_api_key_route).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = request_without_header(&fixture.app, "DELETE", &legacy_api_key_route).await;
    assert_eq!(response.status(), StatusCode::OK);

    let missing_target_route = format!("{self_route}?ApiKey={}", fixture.api_key_token);
    let response = request_without_header(&fixture.app, "POST", &missing_target_route).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let nil_target_route = format!(
        "{self_route}?userId={}&ApiKey={}",
        Uuid::nil(),
        fixture.api_key_token
    );
    let response = request_without_header(&fixture.app, "DELETE", &nil_target_route).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

async fn assert_default_parental_policy(fixture: &PlaystateFixture) {
    let modern_route = modern_playstate_route(fixture.item_id);
    let response = request(
        &fixture.app,
        "POST",
        &modern_route,
        &fixture.blocked_user_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let legacy_route = playstate_route(fixture.blocked_user_id, fixture.item_id);
    let response = request(
        &fixture.app,
        "DELETE",
        &legacy_route,
        &fixture.blocked_user_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

async fn assert_modern_not_found_semantics(fixture: &PlaystateFixture) {
    let missing_user_route = format!(
        "{}?userId={}",
        modern_playstate_route(fixture.item_id),
        Uuid::new_v4()
    );
    for method in ["POST", "DELETE"] {
        let response = request(
            &fixture.app,
            method,
            &missing_user_route,
            &fixture.administrator_token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let missing_item_route = format!(
        "{}?userId={}",
        modern_playstate_route(Uuid::new_v4()),
        fixture.user_id
    );
    for method in ["POST", "DELETE"] {
        let response = request(
            &fixture.app,
            method,
            &missing_item_route,
            &fixture.user_token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

async fn assert_played_and_unplayed(fixture: &PlaystateFixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let mut initial = NewUserData::new(
        fixture.item_id,
        fixture.user_id,
        fixture.item_id.to_string(),
    );
    initial.rating = Some(8.5);
    initial.is_favorite = true;
    initial.playback_position_ticks = 123_456;
    repository.upsert(initial).await.expect("resume seed");

    let modern_route = modern_playstate_route(fixture.item_id);
    let response = request(&fixture.app, "POST", &modern_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let played = body_json(response).await;
    assert_eq!(played["ItemId"], fixture.item_id.simple().to_string());
    assert_eq!(played["Key"], fixture.item_id.to_string());
    assert_eq!(played["Played"], true);
    assert_eq!(played["PlayCount"], 1);
    assert_eq!(played["PlaybackPositionTicks"], 0);
    assert_eq!(played["IsFavorite"], true);
    assert_eq!(played["Rating"], 8.5);
    assert!(played["LastPlayedDate"].is_string());
    assert!(played.get("PlayedPercentage").is_none());
    assert!(played.get("UnplayedItemCount").is_none());

    let dated_route = format!(
        "{modern_route}?userId={}&datePlayed=2026-07-22T10%3A00%3A00Z",
        fixture.user_id
    );
    let response = request(&fixture.app, "POST", &dated_route, &fixture.user_token).await;
    let dated = body_json(response).await;
    assert_eq!(dated["PlayCount"], 2);
    assert_eq!(dated["LastPlayedDate"], "2026-07-22T10:00:00Z");

    let route = playstate_route(fixture.user_id, fixture.item_id);
    let legacy_route = format!("{route}?datePlayed=20260722110000");
    let response = request(&fixture.app, "POST", &legacy_route, &fixture.user_token).await;
    let legacy = body_json(response).await;
    assert_eq!(legacy["PlayCount"], 3);
    assert_eq!(legacy["LastPlayedDate"], "2026-07-22T11:00:00Z");

    let invalid_route = format!("{modern_route}?datePlayed=not-a-date");
    let response = request(&fixture.app, "POST", &invalid_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let modern_delete_route = format!("{modern_route}?userId={}", fixture.user_id);
    let response = request(
        &fixture.app,
        "DELETE",
        &modern_delete_route,
        &fixture.user_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let unplayed = body_json(response).await;
    assert_eq!(unplayed["Played"], false);
    assert_eq!(unplayed["PlayCount"], 0);
    assert_eq!(unplayed["PlaybackPositionTicks"], 0);
    assert!(unplayed.get("LastPlayedDate").is_none());
    assert_eq!(unplayed["IsFavorite"], true);
    assert_eq!(unplayed["Rating"], 8.5);
}

async fn assert_concurrent_manual_play_is_idempotent(fixture: &PlaystateFixture) {
    let route = playstate_route(fixture.user_id, fixture.item_id);
    let (first, second, third, fourth) = tokio::join!(
        request(&fixture.app, "POST", &route, &fixture.user_token),
        request(&fixture.app, "POST", &route, &fixture.user_token),
        request(&fixture.app, "POST", &route, &fixture.user_token),
        request(&fixture.app, "POST", &route, &fixture.user_token),
    );
    for response in [first, second, third, fourth] {
        assert_eq!(response.status(), StatusCode::OK);
    }
    let persisted = UserDataRepository::new(fixture.database.clone())
        .get(
            fixture.item_id,
            fixture.user_id,
            &fixture.item_id.to_string(),
        )
        .await
        .expect("playstate lookup")
        .expect("playstate must exist");
    assert!(persisted.played);
    assert_eq!(persisted.play_count, 1);
    assert_eq!(persisted.playback_position_ticks, 0);
}

fn playstate_route(user_id: Uuid, item_id: Uuid) -> String {
    format!("/Users/{user_id}/PlayedItems/{item_id}")
}

fn modern_playstate_route(item_id: Uuid) -> String {
    format!("/UserPlayedItems/{item_id}")
}

fn blocked_policy() -> UserPolicy {
    UserPolicy {
        access_schedules: vec![AccessSchedule {
            day_of_week: DynamicDayOfWeek::Everyday,
            start_hour: 18.0,
            end_hour: 6.0,
        }],
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
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
                .unwrap(),
        )
        .await
        .unwrap()
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
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn test_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    database
}
