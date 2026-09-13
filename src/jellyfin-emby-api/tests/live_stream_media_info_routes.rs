use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamService, UserService};
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
};
use jellyfin_model::{MediaStream, MediaStreamType};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Live Stream MediaInfo Tests\", DeviceId=\"emby-live-stream-media-info-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_stream_info_";

#[tokio::test]
async fn live_stream_media_info_uses_the_authenticated_global_open_stream_registry() {
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
    assert_authentication_and_binding(&fixture).await;
    let live_stream_id = open_stream(&fixture).await;
    assert_live_stream_lookup_semantics(&fixture, &live_stream_id).await;
    assert_close_makes_the_stream_unavailable(&fixture, &live_stream_id).await;
    assert_jellyfin_root_isolation(&fixture, &live_stream_id).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    user_id: Uuid,
    item_id: Uuid,
    owner_token: String,
    other_session_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("stream-info-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let owner = users
            .create(&format!("stream-info-owner-{suffix}"))
            .await
            .expect("owner creation");
        let other = users
            .create(&format!("stream-info-other-{suffix}"))
            .await
            .expect("other user creation");

        let devices = DeviceRepository::new(database.clone());
        let owner_token = session(&devices, owner.id, "owner").await;
        let other_session_token = session(&devices, other.id, "other").await;
        let _administrator_token = session(&devices, administrator.id, "admin").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("stream-info-api-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.name = Some("Emby opened stream media info".to_owned());
        item.path = Some(format!("/tmp/emby-opened-stream-{suffix}.mkv"));
        BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("media item creation");
        MediaStreamService::new(database.clone())
            .save_media_streams(
                item_id,
                vec![MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    width: Some(1920),
                    height: Some(1080),
                    ..MediaStream::default()
                }],
            )
            .await
            .expect("media stream creation");

        let state = AppState::new(
            database,
            "Emby Live Stream MediaInfo Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let emby = jellyfin_emby_api::router(state.clone());
        let jellyfin = jellyfin_api::router(state);
        Self {
            emby,
            jellyfin,
            user_id: owner.id,
            item_id,
            owner_token,
            other_session_token,
            api_key,
        }
    }
}

async fn assert_authentication_and_binding(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            "/emby/LiveStreams/MediaInfo",
            None,
            None,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "authentication must precede required-query validation",
    );
    for route in [
        "/emby/LiveStreams/MediaInfo",
        "/emby/LiveStreams/MediaInfo?Other=value",
        "/emby/LiveStreams/MediaInfo?LiveStreamId=",
    ] {
        assert_eq!(
            request(
                &fixture.emby,
                Method::POST,
                route,
                Some(&fixture.owner_token),
                None,
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "missing or empty LiveStreamId must be rejected: {route}",
        );
    }
    for route in [
        "/emby/LiveStreams/MediaInfo?LiveStreamId=unknown",
        "/emby/livestreams/mediainfo?livestreamid=%20",
    ] {
        assert_eq!(
            request(
                &fixture.emby,
                Method::POST,
                route,
                Some(&fixture.owner_token),
                None,
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "nonempty unknown stream ids must remain distinct from bad requests: {route}",
        );
    }
}

async fn open_stream(fixture: &Fixture) -> String {
    let response = request(
        &fixture.emby,
        Method::POST,
        "/emby/LiveStreams/Open",
        Some(&fixture.owner_token),
        Some(json!({
            "ItemId": fixture.item_id,
            "UserId": fixture.user_id,
            "PlaySessionId": "owner-play-session",
            "OpenToken": "provider-token"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("open response body"),
    )
    .expect("open response JSON");
    response["MediaSource"]["LiveStreamId"]
        .as_str()
        .expect("opened LiveStreamId")
        .to_owned()
}

async fn assert_live_stream_lookup_semantics(fixture: &Fixture, live_stream_id: &str) {
    let canonical = format!("/emby/LiveStreams/MediaInfo?LiveStreamId={live_stream_id}");
    assert_ok_empty(
        request(
            &fixture.emby,
            Method::POST,
            &canonical,
            Some(&fixture.owner_token),
            None,
        )
        .await,
    )
    .await;

    let mixed_case = format!(
        "/emby/lIvEsTrEaMs/mEdIaInFo?LIVESTREAMID=unknown&lIvEsTrEaMiD={}",
        live_stream_id.to_uppercase()
    );
    assert_ok_empty(
        request(
            &fixture.emby,
            Method::POST,
            &mixed_case,
            Some(&fixture.other_session_token),
            None,
        )
        .await,
    )
    .await;

    // The removed official method receives only the id and reads a global
    // open-stream dictionary, so another authenticated device or an API key
    // can address a known stream. Do not invent a cross-session 403.
    assert_ok_empty(
        request(
            &fixture.emby,
            Method::POST,
            &canonical,
            Some(&fixture.api_key),
            None,
        )
        .await,
    )
    .await;
}

async fn assert_close_makes_the_stream_unavailable(fixture: &Fixture, live_stream_id: &str) {
    let close = format!("/emby/LiveStreams/Close?LiveStreamId={live_stream_id}");
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            &close,
            Some(&fixture.owner_token),
            None,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT,
    );
    assert_eq!(
        request(
            &fixture.emby,
            Method::POST,
            &format!("/emby/LiveStreams/MediaInfo?LiveStreamId={live_stream_id}"),
            Some(&fixture.owner_token),
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "closed and idle-expired entries are absent from the same bounded registry",
    );
}

async fn assert_jellyfin_root_isolation(fixture: &Fixture, live_stream_id: &str) {
    assert_eq!(
        request(
            &fixture.jellyfin,
            Method::POST,
            &format!("/LiveStreams/MediaInfo?LiveStreamId={live_stream_id}"),
            Some(&fixture.owner_token),
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    let body = if let Some(body) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).expect("request JSON"))
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("route response")
}

async fn assert_ok_empty(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("response body")
            .is_empty(),
        "Java and Swift declare this operation's response as Void",
    );
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Live Stream MediaInfo Tests",
            "1.0",
            "Test",
            format!("emby-live-stream-media-info-tests-{suffix}"),
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
