use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Legacy Playstate Tests\", DeviceId=\"emby-legacy-playstate\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_legacy_playstate_";

#[tokio::test]
async fn generated_legacy_playstate_contract_is_isolated_from_jellyfin() {
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
    assert_emby_required_queries_and_body(&fixture).await;
    assert_emby_case_insensitive_last_wins_binding(&fixture).await;
    assert_jellyfin_contract_is_unchanged(&fixture).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    user_id: Uuid,
    item_id: Uuid,
    token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let user = UserService::new(database.clone())
            .create(&format!("emby-legacy-playstate-{suffix}"))
            .await
            .expect("user creation");
        let session = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Legacy Playstate Tests",
                "1.0",
                "Test",
                "emby-legacy-playstate",
            ))
            .await
            .expect("device session");
        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some("Legacy playback item".to_owned());
        item.runtime_ticks = Some(600 * 10_000_000);
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("playback item");
        let state = AppState::new(
            database,
            "Emby Legacy Playstate Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            item_id: item.id,
            token: session.access_token,
        }
    }
}

async fn assert_emby_required_queries_and_body(fixture: &Fixture) {
    let item = format!(
        "/emby/Users/{}/PlayingItems/{}",
        fixture.user_id, fixture.item_id
    );
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            &item,
            &fixture.token,
            Body::empty()
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    let progress = format!("{item}/Progress?MediaSourceId=source");
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            &progress,
            &fixture.token,
            Body::empty()
        )
        .await,
        StatusCode::BAD_REQUEST,
        "the generated progress body is required"
    );

    let stop = format!("{item}?MediaSourceId=source");
    assert_eq!(
        request(
            &fixture.emby,
            Method::DELETE,
            &stop,
            &fixture.token,
            Body::empty()
        )
        .await,
        StatusCode::BAD_REQUEST,
        "NextMediaType is required for generated stop requests"
    );
}

async fn assert_emby_case_insensitive_last_wins_binding(fixture: &Fixture) {
    let item = format!(
        "/emby/uSeRs/{}/pLaYiNgItEmS/{}",
        fixture.user_id, fixture.item_id
    );
    let start = format!(
        "{item}?MediaSourceId=old&mEdIaSoUrCeId=source&CanSeek=invalid&cAnSeEk=TRUE&AudioStreamIndex=bad&aUdIoStReAmInDeX=2"
    );
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            &start,
            &fixture.token,
            Body::empty()
        )
        .await,
        StatusCode::OK
    );

    let progress = format!(
        "{item}/pRoGrEsS?MediaSourceId=old&mEdIaSoUrCeId=source&PositionTicks=bad&pOsItIoNtIcKs=120000000&IsPaused=bad&iSpAuSeD=true&SubtitleOffset=bad&sUbTiTlEoFfSeT=5&PlaybackRate=bad&pLaYbAcKrAtE=1.25"
    );
    let body = Body::from(
        r#"{
            "PlaylistIndex":"bad",
            "pLaYlIsTiNdEx":"2",
            "Shuffle":"bad",
            "sHuFfLe":true,
            "SleepTimerMode":"bad",
            "sLeEpTiMeRmOdE":"AfterItem",
            "EventName":"bad",
            "eVeNtNaMe":"TimeUpdate",
            "UnknownGeneratedField":{"ignored":true}
        }"#,
    );
    assert_eq!(
        request(&fixture.emby, Method::POST, &progress, &fixture.token, body).await,
        StatusCode::OK
    );

    let malformed_body = format!("{item}/Progress?MediaSourceId=source");
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            &malformed_body,
            &fixture.token,
            Body::from(r#"{"PlaylistLength":"bad"}"#)
        )
        .await,
        StatusCode::BAD_REQUEST
    );

    for (method, suffix) in [(Method::DELETE, ""), (Method::POST, "/dElEtE")] {
        let stop = format!(
            "{item}{suffix}?MediaSourceId=old&mEdIaSoUrCeId=source&NextMediaType=old&nExTmEdIaTyPe=Video&PositionTicks=bad&pOsItIoNtIcKs=180000000"
        );
        assert_eq!(
            request(&fixture.emby, method, &stop, &fixture.token, Body::empty()).await,
            StatusCode::OK
        );
    }
}

async fn assert_jellyfin_contract_is_unchanged(fixture: &Fixture) {
    for prefix in ["", "/api"] {
        let item = format!(
            "{prefix}/Users/{}/PlayingItems/{}",
            fixture.user_id, fixture.item_id
        );
        assert_eq!(
            request(
                &fixture.jellyfin,
                Method::POST,
                &item,
                &fixture.token,
                Body::empty()
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            request(
                &fixture.jellyfin,
                Method::POST,
                &format!("{item}/Progress"),
                &fixture.token,
                Body::from("{")
            )
            .await,
            StatusCode::NO_CONTENT,
            "Jellyfin legacy progress must continue to ignore request bodies"
        );
        assert_eq!(
            request(
                &fixture.jellyfin,
                Method::DELETE,
                &item,
                &fixture.token,
                Body::empty()
            )
            .await,
            StatusCode::NO_CONTENT
        );
    }
}

async fn request(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Body,
) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(body)
                .expect("request"),
        )
        .await
        .expect("route response")
        .status()
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
