use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewUserData, UserDataRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Track Selection Tests\", DeviceId=\"emby-track-selection-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_track_selections_";

#[tokio::test]
async fn track_selection_routes_are_authorized_set_based_and_emby_only() {
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
        exercise_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    assert_authorization_and_validation_precedence(&fixture).await;
    assert_audio_clear_is_set_based_and_column_scoped(&fixture).await;
    assert_post_alias_and_elevated_cross_user_access(&fixture).await;
    assert_api_key_cross_user_access(&fixture).await;
    assert_protocol_isolation(&fixture, database.clone()).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    item_ids: [Uuid; 2],
    user_token: String,
    administrator_token: String,
    api_key: String,
    original_user_rows: Vec<jellyfin_data::entities::user_data::Model>,
    original_other_row: jellyfin_data::entities::user_data::Model,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("track-selection-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("track-selection-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("track-selection-other-{suffix}"))
            .await
            .expect("other user creation");

        let items = BaseItemRepository::new(database.clone());
        let first = items
            .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
            .await
            .expect("first movie creation");
        let second = items
            .create(NewBaseItem::new(Uuid::new_v4(), "Episode"))
            .await
            .expect("second video creation");
        let item_ids = [first.id, second.id];
        let user_data = UserDataRepository::new(database.clone());
        let mut original_user_rows = Vec::new();
        for (offset, item_id) in item_ids.into_iter().enumerate() {
            let mut row = NewUserData::new(
                item_id,
                user.id,
                format!("track-selection-key-{offset}-{suffix}"),
            );
            row.rating = Some(7.0 + offset as f64);
            row.playback_position_ticks = 1_000 + offset as i64;
            row.play_count = 2 + offset as i32;
            row.is_favorite = offset == 0;
            row.last_played_date = Some(
                Utc.with_ymd_and_hms(2026, 9, 13, 12 + offset as u32, 0, 0)
                    .unwrap(),
            );
            row.played = offset == 1;
            row.audio_stream_index = Some(10 + offset as i32);
            row.subtitle_stream_index = Some(20 + offset as i32);
            row.likes = Some(offset == 0);
            row.retention_date = Some(
                Utc.with_ymd_and_hms(2026, 10, 1 + offset as u32, 0, 0, 0)
                    .unwrap(),
            );
            original_user_rows.push(user_data.upsert(row).await.expect("user-data seed"));
        }
        let mut other = NewUserData::new(
            item_ids[0],
            other_user.id,
            format!("track-selection-other-key-{suffix}"),
        );
        other.rating = Some(9.0);
        other.playback_position_ticks = 9_999;
        other.play_count = 9;
        other.is_favorite = true;
        other.audio_stream_index = Some(31);
        other.subtitle_stream_index = Some(32);
        let original_other_row = user_data.upsert(other).await.expect("other user-data seed");

        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session(&devices, administrator.id, "admin").await;
        let user_token = session(&devices, user.id, "user").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("track-selection-api-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let app = jellyfin_emby_api::router(AppState::new(
            database.clone(),
            "Emby Track Selection Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            item_ids,
            user_token,
            administrator_token,
            api_key,
            original_user_rows,
            original_other_row,
        }
    }
}

async fn assert_authorization_and_validation_precedence(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            Method::DELETE,
            "/emby/Users/not-a-uuid/TrackSelections/NotATrack",
            None,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "protocol authentication must precede path validation",
    );

    let forbidden = route(fixture.other_user_id, "NotATrack", false);
    assert_eq!(
        request(
            &fixture.app,
            Method::DELETE,
            &forbidden,
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "ordinary users may target only themselves, before TrackType validation",
    );

    let missing = route(Uuid::new_v4(), "NotATrack", false);
    for token in [&fixture.administrator_token, &fixture.api_key] {
        assert_eq!(
            request(&fixture.app, Method::DELETE, &missing, Some(token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "target-user lookup must precede TrackType validation",
        );
    }

    assert_eq!(
        request(
            &fixture.app,
            Method::DELETE,
            &route(fixture.user_id, "Video", false),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
        "only Audio and Subtitle are valid TrackType values",
    );
}

async fn assert_audio_clear_is_set_based_and_column_scoped(fixture: &Fixture) {
    let mixed_case = format!("/emby/uSeRs/{}/tRaCkSeLeCtIoNs/aUdIo", fixture.user_id);
    let response = request(
        &fixture.app,
        Method::DELETE,
        &mixed_case,
        Some(&fixture.user_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1_024)
            .await
            .expect("empty response body")
            .is_empty()
    );

    for (item_id, original) in fixture.item_ids.iter().zip(&fixture.original_user_rows) {
        let updated = stored(&fixture.database, *item_id, fixture.user_id).await;
        assert_eq!(updated.audio_stream_index, None);
        assert_eq!(
            updated.subtitle_stream_index, original.subtitle_stream_index,
            "clearing Audio must preserve Subtitle selections",
        );
        assert_non_selection_fields_equal(original, &updated);
    }
    assert_eq!(
        stored(
            &fixture.database,
            fixture.item_ids[0],
            fixture.other_user_id,
        )
        .await,
        fixture.original_other_row,
        "the set-based update must remain scoped to the target user",
    );
}

async fn assert_post_alias_and_elevated_cross_user_access(fixture: &Fixture) {
    let lowercase_post = route(fixture.user_id, "SUBTITLE", true).to_ascii_lowercase();
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            &lowercase_post,
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::OK,
        "administrators may use the POST compatibility alias for another user",
    );
    for (item_id, original) in fixture.item_ids.iter().zip(&fixture.original_user_rows) {
        let updated = stored(&fixture.database, *item_id, fixture.user_id).await;
        assert_eq!(updated.audio_stream_index, None);
        assert_eq!(updated.subtitle_stream_index, None);
        assert_non_selection_fields_equal(original, &updated);
    }
}

async fn assert_api_key_cross_user_access(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            Method::DELETE,
            &route(fixture.other_user_id, "Audio", false),
            Some(&fixture.api_key),
        )
        .await
        .status(),
        StatusCode::OK,
    );
    let updated = stored(
        &fixture.database,
        fixture.item_ids[0],
        fixture.other_user_id,
    )
    .await;
    assert_eq!(updated.audio_stream_index, None);
    assert_eq!(
        updated.subtitle_stream_index,
        fixture.original_other_row.subtitle_stream_index,
    );
    assert_non_selection_fields_equal(&fixture.original_other_row, &updated);
}

async fn assert_protocol_isolation(fixture: &Fixture, database: DatabaseConnection) {
    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Track Selection Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    for (method, suffix) in [
        (Method::DELETE, "TrackSelections/Audio"),
        (Method::POST, "TrackSelections/Audio/Delete"),
    ] {
        assert_eq!(
            request(
                &jellyfin,
                method,
                &format!("/Users/{}/{suffix}", fixture.user_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "TrackSelections compatibility routes must stay inside /emby",
        );
    }
}

async fn stored(
    database: &DatabaseConnection,
    item_id: Uuid,
    user_id: Uuid,
) -> jellyfin_data::entities::user_data::Model {
    UserDataRepository::new(database.clone())
        .get_for_item(item_id, user_id)
        .await
        .expect("user-data query")
        .into_iter()
        .next()
        .expect("persisted user-data row")
}

fn assert_non_selection_fields_equal(
    expected: &jellyfin_data::entities::user_data::Model,
    actual: &jellyfin_data::entities::user_data::Model,
) {
    assert_eq!(actual.item_id, expected.item_id);
    assert_eq!(actual.user_id, expected.user_id);
    assert_eq!(actual.custom_data_key, expected.custom_data_key);
    assert_eq!(actual.rating, expected.rating);
    assert_eq!(
        actual.playback_position_ticks,
        expected.playback_position_ticks
    );
    assert_eq!(actual.play_count, expected.play_count);
    assert_eq!(actual.is_favorite, expected.is_favorite);
    assert_eq!(actual.last_played_date, expected.last_played_date);
    assert_eq!(actual.played, expected.played);
    assert_eq!(actual.likes, expected.likes);
    assert_eq!(actual.retention_date, expected.retention_date);
    assert_eq!(actual.is_hidden_from_resume, expected.is_hidden_from_resume);
}

fn route(user_id: Uuid, track_type: &str, post_alias: bool) -> String {
    let suffix = if post_alias { "/Delete" } else { "" };
    format!("/emby/Users/{user_id}/TrackSelections/{track_type}{suffix}")
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Track Selection Tests",
            "1.0",
            "Test",
            format!("emby-track-selection-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
