use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamService, UserService};
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice,
    entities::{base_item, user},
};
use jellyfin_model::{MediaStream, MediaStreamType};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Media Info Tests\", DeviceId=\"media-info-tests\", Device=\"Test\", Version=\"1.0\"";
const DEFAULT_SIZE: usize = 102_400;
const MAX_SIZE: usize = 100_000_000;
const REPEATING_BLOCK_SIZE: usize = 4 * 1024;

#[tokio::test]
async fn official_bitrate_test_default_and_valid_size_contract() {
    let fixture = Fixture::new().await;
    for uri in ["/Playback/BitrateTest", "/Playback/BitrateTest?size=102400"] {
        let response = fixture.get(uri, Some(&fixture.admin_token)).await;
        assert_bitrate_headers(&response, DEFAULT_SIZE);
        let body = to_bytes(response.into_body(), DEFAULT_SIZE + 1)
            .await
            .unwrap();
        assert_eq!(body.len(), DEFAULT_SIZE, "{uri}");
        assert!(body[..32].windows(2).any(|bytes| bytes[0] != bytes[1]));
        assert_eq!(
            &body[..DEFAULT_SIZE - REPEATING_BLOCK_SIZE],
            &body[REPEATING_BLOCK_SIZE..],
            "{uri}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn official_bitrate_test_invalid_values_and_parse_boundaries() {
    let fixture = Fixture::new().await;
    for size in [
        "0",
        "-102400",
        "1000000000",
        "100000001",
        "not-a-number",
        "999999999999999999999999999999999999",
    ] {
        let uri = format!("/Playback/BitrateTest?size={size}");
        assert_eq!(
            fixture.get(&uri, Some(&fixture.admin_token)).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn bitrate_test_authentication_and_inclusive_bounds() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.get("/Playback/BitrateTest", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let response = fixture
        .get("/Playback/BitrateTest?size=1", Some(&fixture.user_token))
        .await;
    assert_bitrate_headers(&response, 1);
    assert_eq!(to_bytes(response.into_body(), 2).await.unwrap().len(), 1);

    let response = fixture
        .get(
            "/Playback/BitrateTest?size=100000000",
            Some(&fixture.user_token),
        )
        .await;
    assert_bitrate_headers(&response, MAX_SIZE);
    drop(response);
    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_info_routes_return_postgres_media_sources_with_official_auth_shape() {
    let fixture = Fixture::new().await;
    let route = format!("/Items/{}/PlaybackInfo", fixture.item_id);

    assert_eq!(
        fixture.get(&route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Items/{}/PlaybackInfo",
                    Uuid::from_u128(0xdddd_dddd_dddd_dddd_dddd_dddd_dddd_dddd)
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("{route}?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let response = fixture.get(&route, Some(&fixture.user_token)).await;
    let playback = body_json(response).await;
    assert_playback_info(&playback, &fixture);

    let empty_post = fixture.post(&route, Some(&fixture.user_token), None).await;
    assert_playback_info(&body_json(empty_post).await, &fixture);

    let post_with_body_and_query = fixture
        .post(
            &format!("{route}?mediaSourceId={}", fixture.item_id),
            Some(&fixture.user_token),
            Some(&json!({
                "UserId": fixture.user_id,
                "MediaSourceId": "ignored-by-query"
            })),
        )
        .await;
    assert_playback_info(&body_json(post_with_body_and_query).await, &fixture);

    fixture.cleanup().await;
}

#[tokio::test]
async fn live_stream_routes_open_postgres_media_sources_and_close_by_required_id() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.post("/LiveStreams/Open", None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .post("/LiveStreams/Open", Some(&fixture.user_token), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .post(
                &format!(
                    "/LiveStreams/Open?itemId={}&userId={}",
                    fixture.item_id, fixture.admin_id
                ),
                Some(&fixture.user_token),
                None,
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let open = body_json(
        fixture
            .post(
                "/LiveStreams/Open",
                Some(&fixture.user_token),
                Some(&json!({
                    "ItemId": fixture.item_id,
                    "UserId": fixture.user_id,
                    "PlaySessionId": "body-session",
                    "OpenToken": "body-token"
                })),
            )
            .await,
    )
    .await;
    assert_live_stream(&open, &fixture, "body-session", "body-token");

    let query_wins = body_json(
        fixture
            .post(
                &format!(
                    "/LiveStreams/Open?itemId={}&playSessionId=query-session&openToken=query-token",
                    fixture.item_id
                ),
                Some(&fixture.user_token),
                Some(&json!({
                    "ItemId": Uuid::from_u128(0xeeee_eeee_eeee_eeee_eeee_eeee_eeee_eeee),
                    "PlaySessionId": "ignored-session",
                    "OpenToken": "ignored-token"
                })),
            )
            .await,
    )
    .await;
    assert_live_stream(&query_wins, &fixture, "query-session", "query-token");

    assert_eq!(
        fixture
            .post("/LiveStreams/Close", Some(&fixture.user_token), None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .post(
                "/LiveStreams/Close?liveStreamId=%20",
                Some(&fixture.user_token),
                None,
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .post(
                "/LiveStreams/Close?liveStreamId=body-session",
                Some(&fixture.user_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    fixture.cleanup().await;
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

fn assert_live_stream(
    live_stream: &Value,
    fixture: &Fixture,
    play_session_id: &str,
    open_token: &str,
) {
    let source = &live_stream["MediaSource"];
    assert_eq!(source["Id"], fixture.item_id.simple().to_string());
    assert_eq!(source["Protocol"], "File");
    assert_eq!(source["Path"], fixture.item_path);
    assert_eq!(source["RequiresOpening"], false);
    assert_eq!(source["RequiresClosing"], true);
    assert_eq!(
        source["LiveStreamId"],
        format!(
            "{}:{play_session_id}:{open_token}",
            fixture.item_id.simple()
        )
    );
    assert_eq!(source["MediaStreams"][0]["Codec"], "h264");
    assert_eq!(source["MediaStreams"][1]["Codec"], "aac");
}

fn assert_playback_info(playback: &Value, fixture: &Fixture) {
    assert_eq!(
        playback["PlaySessionId"]
            .as_str()
            .expect("play session id")
            .len(),
        32
    );
    assert!(playback.get("ErrorCode").is_none());
    let sources = playback["MediaSources"]
        .as_array()
        .expect("media sources array");
    assert_eq!(sources.len(), 1);
    let source = &sources[0];
    assert_eq!(source["Id"], fixture.item_id.simple().to_string());
    assert_eq!(source["Protocol"], "File");
    assert_eq!(source["Path"], fixture.item_path);
    assert!(
        source["Name"]
            .as_str()
            .expect("media source name")
            .starts_with("playback-info-movie-")
    );
    assert_eq!(source["Container"], "mkv");
    assert_eq!(source["RunTimeTicks"], 12_345_000_000_i64);
    assert_eq!(source["SupportsDirectPlay"], true);
    assert_eq!(source["SupportsDirectStream"], true);
    assert_eq!(source["SupportsTranscoding"], true);
    assert_eq!(source["MediaStreams"][0]["Index"], 0);
    assert_eq!(source["MediaStreams"][0]["Type"], 1);
    assert_eq!(source["MediaStreams"][0]["Codec"], "h264");
    assert_eq!(source["MediaStreams"][0]["Width"], 1920);
    assert_eq!(source["MediaStreams"][0]["Height"], 1080);
    assert_eq!(source["MediaStreams"][1]["Index"], 1);
    assert_eq!(source["MediaStreams"][1]["Type"], 0);
    assert_eq!(source["MediaStreams"][1]["Codec"], "aac");
    assert_eq!(source["MediaStreams"][1]["Language"], "eng");
}

fn assert_bitrate_headers(response: &axum::response::Response, expected_size: usize) {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        expected_size.to_string()
    );
    assert!(!response.headers().contains_key(header::TRANSFER_ENCODING));
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    user_id: Uuid,
    item_id: Uuid,
    item_path: String,
    admin_token: String,
    user_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("media-info-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("media-info-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("media-info-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("media-info-user-{suffix}")).await;
        let item_path = format!("/media/playback-info-movie-{suffix}.mkv");
        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.name = Some("playback-info-movie".to_owned());
        item.path = Some(item_path.clone());
        item.runtime_ticks = Some(12_345_000_000);
        BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("playback info item creation");
        MediaStreamService::new(database.clone())
            .save_media_streams(
                item_id,
                &[
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Video,
                        codec: Some("h264".to_owned()),
                        width: Some(1920),
                        height: Some(1080),
                        is_default: true,
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("eng".to_owned()),
                        channels: Some(2),
                        is_default: true,
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("playback info media stream creation");
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Media Info Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            user_id: user.id,
            item_id,
            item_path,
            admin_token,
            user_token,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        self.request(Method::GET, uri, token, None).await
    }

    async fn post(
        &self,
        uri: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> axum::response::Response {
        self.request(Method::POST, uri, token, body).await
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<&Value>,
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
            Body::from(serde_json::to_vec(body).unwrap())
        } else {
            Body::empty()
        };
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_by_id(self.item_id)
            .exec(&self.database)
            .await
            .expect("media info item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("media info user cleanup");
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Media Info Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("media info session")
        .access_token
}
