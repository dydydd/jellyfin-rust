use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemQuery, BaseItemRepository, DatabaseConfig, DeviceRepository,
    NewBaseItem, NewDevice, NewUserData, UserDataRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Hide Resume Tests\", DeviceId=\"emby-hide-resume-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_hide_resume_";

#[tokio::test]
async fn hide_from_resume_is_authorized_reversible_and_postgres_backed() {
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
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    assert_binding_and_authorization(&fixture).await;
    assert_reversible_persistence(&fixture).await;
    assert_administrator_and_api_key_targets(&fixture).await;
    assert_protocol_isolation(&fixture, database.clone()).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    item_id: Uuid,
    user_token: String,
    administrator_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("hide-resume-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("hide-resume-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("hide-resume-other-{suffix}"))
            .await
            .expect("other user creation");

        let items = BaseItemRepository::new(database.clone());
        let mut movie = NewBaseItem::new(Uuid::new_v4(), "Episode");
        movie.name = Some("Reversible resume episode".to_owned());
        movie.runtime_ticks = Some(10_000);
        movie.parent_index_number = Some(1);
        movie.index_number = Some(1);
        movie.series_presentation_unique_key = Some(format!("hide-resume-series-{suffix}"));
        let movie = items.create(movie).await.expect("episode creation");

        let mut data = NewUserData::new(movie.id, user.id, movie.id.simple().to_string());
        data.rating = Some(8.0);
        data.likes = Some(true);
        data.playback_position_ticks = 2_500;
        data.play_count = 7;
        data.is_favorite = true;
        data.last_played_date = Some(Utc.with_ymd_and_hms(2026, 9, 13, 12, 0, 0).unwrap());
        UserDataRepository::new(database.clone())
            .upsert(data)
            .await
            .expect("resume state seed");

        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session(&devices, administrator.id, "admin").await;
        let user_token = session(&devices, user.id, "user").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("hide-resume-api-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let app = jellyfin_emby_api::router(AppState::new(
            database.clone(),
            "Emby Hide Resume Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            item_id: movie.id,
            user_token,
            administrator_token,
            api_key,
        }
    }
}

async fn assert_binding_and_authorization(fixture: &Fixture) {
    let canonical = route(fixture.user_id, fixture.item_id, "Hide=true");
    assert_eq!(
        request(&fixture.app, &canonical, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let other_target = route(fixture.other_user_id, fixture.item_id, "Hide=invalid");
    assert_eq!(
        request(&fixture.app, &other_target, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "target authorization must precede malformed query binding"
    );
    assert_eq!(
        request(
            &fixture.app,
            &route(fixture.user_id, fixture.item_id, "Other=true"),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &fixture.app,
            &route(fixture.user_id, fixture.item_id, "Hide=not-a-bool"),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &fixture.app,
            &route(fixture.user_id, Uuid::new_v4(), "Hide=true"),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_reversible_persistence(fixture: &Fixture) {
    assert_resume_visibility(fixture, true).await;
    let before = stored(fixture).await;

    let mixed_case = format!(
        "/emby/uSeRs/{}/iTeMs/{}/hIdEfRoMrEsUmE?hIdE=TrUe",
        fixture.user_id, fixture.item_id
    );
    let response = request(&fixture.app, &mixed_case, Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["PlaybackPositionTicks"], 2_500);
    assert_eq!(body["PlayCount"], 7);
    assert_eq!(body["IsFavorite"], true);
    assert_eq!(body["Rating"], 8.0);
    assert!(body.get("IsHiddenFromResume").is_none());

    let hidden = stored(fixture).await;
    assert!(hidden.is_hidden_from_resume);
    assert_ordinary_fields_equal(&before, &hidden);
    assert_resume_visibility(fixture, false).await;

    let lowercase = format!(
        "/emby/users/{}/items/{}/hidefromresume?HIDE=fAlSe",
        fixture.user_id, fixture.item_id
    );
    let response = request(&fixture.app, &lowercase, Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let restored = stored(fixture).await;
    assert!(!restored.is_hidden_from_resume);
    assert_ordinary_fields_equal(&before, &restored);
    assert_resume_visibility(fixture, true).await;
}

async fn assert_administrator_and_api_key_targets(fixture: &Fixture) {
    let hide_route = route(fixture.user_id, fixture.item_id, "Hide=true");
    assert_eq!(
        request(
            &fixture.app,
            &hide_route,
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let restore = route(fixture.user_id, fixture.item_id, "Hide=false");
    assert_eq!(
        request(&fixture.app, &restore, Some(&fixture.api_key))
            .await
            .status(),
        StatusCode::OK
    );

    let missing_user = Uuid::new_v4();
    let missing_target = route(missing_user, fixture.item_id, "Hide=true");
    for token in [&fixture.administrator_token, &fixture.api_key] {
        assert_eq!(
            request(&fixture.app, &missing_target, Some(token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "elevated callers must not create user data for an unknown target"
        );
    }
}

async fn assert_protocol_isolation(fixture: &Fixture, database: DatabaseConnection) {
    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    assert_eq!(
        request(
            &jellyfin,
            &format!(
                "/Users/{}/Items/{}/HideFromResume?Hide=true",
                fixture.user_id, fixture.item_id
            ),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_resume_visibility(fixture: &Fixture, expected: bool) {
    let repository = BaseItemRepository::new(fixture.database.clone());
    let page = repository
        .query_resumable(
            fixture.user_id,
            &BaseItemQuery {
                ids: vec![fixture.item_id],
                ..Default::default()
            },
        )
        .await
        .expect("resume query");
    assert_eq!(
        page.items.iter().any(|item| item.id == fixture.item_id),
        expected
    );
    assert_eq!(page.total_record_count, u64::from(expected));
    let next_up = repository
        .next_up(
            fixture.user_id,
            &BaseItemQuery::default(),
            false,
            true,
            true,
            None,
            0,
            None,
        )
        .await
        .expect("NextUp query");
    assert_eq!(
        next_up.items.iter().any(|item| item.id == fixture.item_id),
        expected
    );
}

async fn stored(fixture: &Fixture) -> jellyfin_data::entities::user_data::Model {
    UserDataRepository::new(fixture.database.clone())
        .get_for_item(fixture.item_id, fixture.user_id)
        .await
        .expect("user data lookup")
        .into_iter()
        .next()
        .expect("persisted user data")
}

fn assert_ordinary_fields_equal(
    expected: &jellyfin_data::entities::user_data::Model,
    actual: &jellyfin_data::entities::user_data::Model,
) {
    assert_eq!(actual.rating, expected.rating);
    assert_eq!(
        actual.playback_position_ticks,
        expected.playback_position_ticks
    );
    assert_eq!(actual.play_count, expected.play_count);
    assert_eq!(actual.is_favorite, expected.is_favorite);
    assert_eq!(actual.last_played_date, expected.last_played_date);
    assert_eq!(actual.played, expected.played);
    assert_eq!(actual.audio_stream_index, expected.audio_stream_index);
    assert_eq!(actual.subtitle_stream_index, expected.subtitle_stream_index);
    assert_eq!(actual.likes, expected.likes);
    assert_eq!(actual.retention_date, expected.retention_date);
}

fn route(user_id: Uuid, item_id: Uuid, query: &str) -> String {
    format!("/emby/Users/{user_id}/Items/{item_id}/HideFromResume?{query}")
}

async fn request(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().method("POST").uri(uri);
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

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Hide Resume Tests",
            "1.0",
            "Test",
            format!("emby-hide-resume-tests-{suffix}"),
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
