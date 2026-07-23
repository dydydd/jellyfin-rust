use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, NewUserData,
    UserDataRepository,
    entities::{api_key, user},
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, UserPolicy};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Playstate Tests\", Device=\"Test\", DeviceId=\"playstate\", Version=\"1.0\"";
const TICKS_PER_SECOND: i64 = 10_000_000;

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

#[tokio::test]
async fn playback_progress_json_route_updates_resume_and_stream_choices() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Progress",
        &fixture.user_token,
        json!({
            "ItemId": fixture.runtime_item_id,
            "PositionTicks": ticks(300),
            "AudioStreamIndex": 2,
            "SubtitleStreamIndex": 3
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_progress(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        ticks(300),
        Some(2),
        Some(3),
        false,
    )
    .await;

    set_stream_remembering(&fixture.database, fixture.user_id, false).await;
    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Progress",
        &fixture.user_token,
        json!({
            "ItemId": fixture.runtime_item_id,
            "PositionTicks": ticks(301),
            "AudioStreamIndex": 4,
            "SubtitleStreamIndex": 5
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_progress(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        ticks(301),
        None,
        None,
        false,
    )
    .await;

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Progress",
        &fixture.user_token,
        json!({
            "ItemId": fixture.item_id,
            "PositionTicks": 1
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_progress(
        &repository,
        fixture.item_id,
        fixture.user_id,
        0,
        None,
        None,
        true,
    )
    .await;

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Progress",
        &fixture.user_token,
        json!({
            "ItemId": fixture.runtime_item_id,
            "PositionTicks": -1
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_progress_legacy_route_uses_authenticated_user_and_missing_items_are_noops() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());

    set_stream_remembering(&fixture.database, fixture.user_id, true).await;
    let legacy_route = format!(
        "/Users/{}/PlayingItems/{}/Progress?positionTicks={}&audioStreamIndex=6&subtitleStreamIndex=-1",
        fixture.administrator_id,
        fixture.runtime_item_id,
        ticks(300)
    );
    let response = request(&fixture.app, "POST", &legacy_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_progress(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        ticks(300),
        Some(6),
        Some(-1),
        false,
    )
    .await;
    assert!(
        repository
            .get(
                fixture.runtime_item_id,
                fixture.administrator_id,
                &fixture.runtime_item_id.to_string()
            )
            .await
            .expect("administrator progress lookup")
            .is_none(),
        "legacy userId route parameter is ignored by Jellyfin"
    );

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Progress",
        &fixture.user_token,
        json!({
            "ItemId": Uuid::new_v4(),
            "PositionTicks": 999_999
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_start_routes_increment_play_count_and_preserve_existing_state() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());
    let mut initial = NewUserData::new(
        fixture.item_id,
        fixture.user_id,
        fixture.item_id.to_string(),
    );
    initial.play_count = 2;
    initial.playback_position_ticks = 44_000;
    initial.is_favorite = true;
    initial.audio_stream_index = Some(1);
    initial.subtitle_stream_index = Some(-1);
    repository.upsert(initial).await.expect("start seed");

    let before_start = Utc::now();
    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing",
        &fixture.user_token,
        json!({ "ItemId": fixture.item_id }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_started(
        &repository,
        fixture.item_id,
        fixture.user_id,
        3,
        before_start,
    )
    .await;

    let legacy_route = format!(
        "/Users/{}/PlayingItems/{}",
        fixture.administrator_id, fixture.item_id
    );
    let before_legacy = Utc::now();
    let response = request(&fixture.app, "POST", &legacy_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_started(
        &repository,
        fixture.item_id,
        fixture.user_id,
        4,
        before_legacy,
    )
    .await;
    assert!(
        repository
            .get(
                fixture.item_id,
                fixture.administrator_id,
                &fixture.item_id.to_string()
            )
            .await
            .expect("administrator start lookup")
            .is_none(),
        "legacy start userId route parameter is ignored by Jellyfin"
    );

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing",
        &fixture.user_token,
        json!({ "ItemId": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_started(
        &repository,
        fixture.item_id,
        fixture.user_id,
        4,
        before_legacy,
    )
    .await;

    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_stopped_no_position_failed_negative_and_missing_items() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());
    let original_last_played = chrono::DateTime::parse_from_rfc3339("2026-07-22T10:00:00Z")
        .expect("fixed date")
        .with_timezone(&Utc);
    let mut initial = NewUserData::new(
        fixture.item_id,
        fixture.user_id,
        fixture.item_id.to_string(),
    );
    initial.play_count = 5;
    initial.playback_position_ticks = ticks(123);
    initial.is_favorite = true;
    initial.last_played_date = Some(original_last_played);
    initial.audio_stream_index = Some(1);
    initial.subtitle_stream_index = Some(-1);
    repository.upsert(initial).await.expect("stop seed");

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({
            "ItemId": fixture.item_id,
            "PositionTicks": ticks(5),
            "Failed": true
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_stopped(
        &repository,
        fixture.item_id,
        fixture.user_id,
        5,
        ticks(123),
        false,
    )
    .await;

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({
            "ItemId": fixture.item_id,
            "PositionTicks": -1
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_stopped(
        &repository,
        fixture.item_id,
        fixture.user_id,
        5,
        ticks(123),
        false,
    )
    .await;

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({ "ItemId": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_stopped(
        &repository,
        fixture.item_id,
        fixture.user_id,
        5,
        ticks(123),
        false,
    )
    .await;

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({ "ItemId": fixture.item_id }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let persisted = assert_stopped(&repository, fixture.item_id, fixture.user_id, 6, 0, true).await;
    assert_eq!(persisted.last_played_date, Some(original_last_played));
    assert!(persisted.is_favorite);
    assert_eq!(persisted.audio_stream_index, Some(1));
    assert_eq!(persisted.subtitle_stream_index, Some(-1));

    fixture.cleanup().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn playback_stopped_position_routes_apply_thresholds_and_legacy_user() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());
    let mut initial = NewUserData::new(
        fixture.runtime_item_id,
        fixture.user_id,
        fixture.runtime_item_id.to_string(),
    );
    initial.play_count = 2;
    initial.is_favorite = true;
    initial.audio_stream_index = Some(7);
    repository
        .upsert(initial.clone())
        .await
        .expect("runtime seed");

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({
            "ItemId": fixture.runtime_item_id,
            "PositionTicks": ticks(300)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let persisted = assert_stopped(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        2,
        ticks(300),
        false,
    )
    .await;
    assert!(persisted.is_favorite);
    assert_eq!(persisted.audio_stream_index, Some(7));

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({
            "ItemId": fixture.runtime_item_id,
            "PositionTicks": ticks(590)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_stopped(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        2,
        0,
        true,
    )
    .await;

    repository
        .upsert(initial)
        .await
        .expect("reset runtime seed");
    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Stopped",
        &fixture.user_token,
        json!({
            "ItemId": fixture.runtime_item_id,
            "PositionTicks": ticks(20)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_stopped(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        2,
        0,
        false,
    )
    .await;

    let legacy_route = format!(
        "/Users/{}/PlayingItems/{}?positionTicks={}",
        fixture.administrator_id,
        fixture.runtime_item_id,
        ticks(300)
    );
    let response = request(&fixture.app, "DELETE", &legacy_route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_stopped(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        2,
        ticks(300),
        false,
    )
    .await;
    assert!(
        repository
            .get(
                fixture.runtime_item_id,
                fixture.administrator_id,
                &fixture.runtime_item_id.to_string()
            )
            .await
            .expect("administrator stop lookup")
            .is_none(),
        "legacy stopped userId route parameter is ignored by Jellyfin"
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_completion_propagates_to_alternate_versions() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());
    let mut primary = NewUserData::new(
        fixture.runtime_item_id,
        fixture.user_id,
        fixture.runtime_item_id.to_string(),
    );
    primary.play_count = 4;
    primary.playback_position_ticks = ticks(120);
    primary.is_favorite = true;
    repository.upsert(primary).await.expect("primary seed");

    let response = request_json(
        &fixture.app,
        "POST",
        "/Sessions/Playing/Progress",
        &fixture.user_token,
        json!({
            "ItemId": fixture.alternate_item_id,
            "PositionTicks": ticks(590)
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_stopped(
        &repository,
        fixture.alternate_item_id,
        fixture.user_id,
        0,
        0,
        true,
    )
    .await;
    let propagated = assert_stopped(
        &repository,
        fixture.runtime_item_id,
        fixture.user_id,
        4,
        0,
        true,
    )
    .await;
    assert!(propagated.is_favorite);

    fixture.cleanup().await;
}

#[tokio::test]
async fn manual_played_unplayed_routes_propagate_to_alternate_versions() {
    let fixture = PlaystateFixture::new().await;
    let repository = UserDataRepository::new(fixture.database.clone());
    let original_last_played = chrono::DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z")
        .expect("fixed date")
        .with_timezone(&Utc);
    let mut alternate = NewUserData::new(
        fixture.alternate_item_id,
        fixture.user_id,
        fixture.alternate_item_id.to_string(),
    );
    alternate.play_count = 7;
    alternate.playback_position_ticks = ticks(240);
    alternate.is_favorite = true;
    alternate.last_played_date = Some(original_last_played);
    repository.upsert(alternate).await.expect("alternate seed");

    let response = request(
        &fixture.app,
        "POST",
        &modern_playstate_route(fixture.runtime_item_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let played = assert_stopped(
        &repository,
        fixture.alternate_item_id,
        fixture.user_id,
        7,
        0,
        true,
    )
    .await;
    assert_eq!(played.last_played_date, Some(original_last_played));
    assert!(played.is_favorite);

    let response = request(
        &fixture.app,
        "DELETE",
        &modern_playstate_route(fixture.runtime_item_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let unplayed = assert_stopped(
        &repository,
        fixture.alternate_item_id,
        fixture.user_id,
        0,
        0,
        false,
    )
    .await;
    assert!(unplayed.last_played_date.is_none());
    assert!(unplayed.is_favorite);

    fixture.cleanup().await;
}

async fn assert_progress(
    repository: &UserDataRepository,
    item_id: Uuid,
    user_id: Uuid,
    position_ticks: i64,
    audio_stream_index: Option<i32>,
    subtitle_stream_index: Option<i32>,
    played: bool,
) {
    let persisted = repository
        .get(item_id, user_id, &item_id.to_string())
        .await
        .expect("progress lookup")
        .expect("progress row");
    assert_eq!(persisted.playback_position_ticks, position_ticks);
    assert_eq!(persisted.audio_stream_index, audio_stream_index);
    assert_eq!(persisted.subtitle_stream_index, subtitle_stream_index);
    assert_eq!(persisted.played, played);
}

async fn assert_started(
    repository: &UserDataRepository,
    item_id: Uuid,
    user_id: Uuid,
    play_count: i32,
    not_before: chrono::DateTime<Utc>,
) {
    let persisted = repository
        .get(item_id, user_id, &item_id.to_string())
        .await
        .expect("start lookup")
        .expect("start row");
    assert_eq!(persisted.play_count, play_count);
    let lower_bound = not_before - chrono::Duration::seconds(1);
    assert!(
        persisted
            .last_played_date
            .is_some_and(|date| date >= lower_bound),
        "last played date should be refreshed"
    );
    assert!(!persisted.played);
    assert_eq!(persisted.playback_position_ticks, 44_000);
    assert!(persisted.is_favorite);
    assert_eq!(persisted.audio_stream_index, Some(1));
    assert_eq!(persisted.subtitle_stream_index, Some(-1));
}

async fn assert_stopped(
    repository: &UserDataRepository,
    item_id: Uuid,
    user_id: Uuid,
    play_count: i32,
    playback_position_ticks: i64,
    played: bool,
) -> jellyfin_data::entities::user_data::Model {
    let persisted = repository
        .get(item_id, user_id, &item_id.to_string())
        .await
        .expect("stop lookup")
        .expect("stop row");
    assert_eq!(persisted.play_count, play_count);
    assert_eq!(persisted.playback_position_ticks, playback_position_ticks);
    assert_eq!(persisted.played, played);
    persisted
}

const fn ticks(seconds: i64) -> i64 {
    seconds * TICKS_PER_SECOND
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
    runtime_item_id: Uuid,
    alternate_item_id: Uuid,
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
        let mut runtime_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        runtime_item.runtime_ticks = Some(ticks(600));
        let runtime_item = items
            .create(runtime_item)
            .await
            .expect("runtime item creation");
        let mut alternate_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        alternate_item.runtime_ticks = Some(ticks(600));
        alternate_item.primary_version_id = Some(runtime_item.id);
        let alternate_item = items
            .create(alternate_item)
            .await
            .expect("alternate item creation");
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
            runtime_item_id: runtime_item.id,
            alternate_item_id: alternate_item.id,
        }
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API key cleanup");
        BaseItemRepository::new(self.database.clone())
            .delete_many(&[self.item_id, self.runtime_item_id, self.alternate_item_id])
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

async fn set_stream_remembering(database: &DatabaseConnection, user_id: Uuid, remember: bool) {
    user::ActiveModel {
        id: Set(user_id),
        preferences: Set(json!({
            "RememberAudioSelections": remember,
            "RememberSubtitleSelections": remember,
            "EnableNextEpisodeAutoPlay": true
        })),
        ..Default::default()
    }
    .update(database)
    .await
    .expect("stream remembering preference update");
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

async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Value,
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
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
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
