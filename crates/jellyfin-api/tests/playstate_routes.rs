use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, NewUserData, UserDataRepository,
    entities::user,
};
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
            item_id: item.id,
        }
    }

    async fn cleanup(self) {
        BaseItemRepository::new(self.database.clone())
            .delete(self.item_id)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.user_id]))
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

    let route = playstate_route(fixture.user_id, fixture.item_id);
    let response = request(&fixture.app, "POST", &route, &fixture.user_token).await;
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
    assert!(played["PlayedPercentage"].is_null());

    let dated_route = format!("{route}?datePlayed=2026-07-22T10%3A00%3A00Z");
    let response = request(&fixture.app, "POST", &dated_route, &fixture.user_token).await;
    let dated = body_json(response).await;
    assert_eq!(dated["PlayCount"], 2);
    assert_eq!(dated["LastPlayedDate"], "2026-07-22T10:00:00Z");

    let legacy_route = format!("{route}?datePlayed=20260722110000");
    let response = request(&fixture.app, "POST", &legacy_route, &fixture.user_token).await;
    let legacy = body_json(response).await;
    assert_eq!(legacy["PlayCount"], 3);
    assert_eq!(legacy["LastPlayedDate"], "2026-07-22T11:00:00Z");

    let invalid_route = format!("{route}?datePlayed=not-a-date");
    let response = request(&fixture.app, "POST", &invalid_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = request(&fixture.app, "DELETE", &route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let unplayed = body_json(response).await;
    assert_eq!(unplayed["Played"], false);
    assert_eq!(unplayed["PlayCount"], 0);
    assert_eq!(unplayed["PlaybackPositionTicks"], 0);
    assert!(unplayed["LastPlayedDate"].is_null());
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
