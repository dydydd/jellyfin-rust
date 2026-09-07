#![allow(clippy::too_many_lines)]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice,
    entities::{api_key, user},
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, MimeTypes, UserPolicy};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str =
    "MediaBrowser Client=\"HLS Tests\", DeviceId=\"hls-tests\", Device=\"Test\", Version=\"1.0\"";
const TRANSCODE_DIRECTORY_NAME: &str = "jellyfin-hls-segment-tests";

#[tokio::test]
async fn official_audio_rows_are_anonymous_path_safe_and_stream_real_files() {
    let fixture = Fixture::new().await;
    fs::write(fixture.transcode_path().join("segment.mp3"), b"0123456789").unwrap();
    fs::write(
        fixture.transcode_path().join("audio-segment.aac"),
        b"aac-body",
    )
    .unwrap();

    // Official valid row.
    let response = fixture
        .get("/Audio/abc/hls/segment/stream.mp3", HeaderMap::new())
        .await;
    assert_file_response(response, StatusCode::OK, "audio/mpeg", b"0123456789").await;

    // The second official route is equally anonymous and uses Jellyfin MIME mapping.
    let response = fixture
        .get(
            "/Audio/not-a-guid/hls/audio-segment/stream.aac",
            HeaderMap::new(),
        )
        .await;
    assert_file_response(response, StatusCode::OK, "audio/aac", b"aac-body").await;

    // Official traversal theory rows, rooted path row, and sibling-prefix row.
    let sibling =
        format!("/Audio/abc/hls/%2E%2E%2F{TRANSCODE_DIRECTORY_NAME}-evil%2Fpasswd/stream.mp3");
    for uri in [
        "/Audio/abc/hls/%2E%2E%2F%2E%2E%2F%2E%2E%2F%2E%2E%2Fetc%2Fpasswd/stream.mp3",
        "/Audio/abc/hls/subdir%2F%2E%2E%2F%2E%2E%2F%2E%2E%2F%2E%2E%2Fetc%2Fpasswd/stream.mp3",
        "/Audio/abc/hls/%2Fetc%2Fpasswd/stream.mp3",
        &sibling,
    ] {
        assert_eq!(
            fixture.get(uri, HeaderMap::new()).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }

    let response = fixture
        .get("/Audio/abc/hls/missing/stream.mp3", HeaderMap::new())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=2-5"));
    let response = fixture
        .get("/Audio/abc/hls/segment/stream.mp3", headers)
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/mpeg");
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(body(response).await, b"2345");

    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-2"));
    headers.insert(header::IF_RANGE, HeaderValue::from_static("\"stale-etag\""));
    let response = fixture
        .get("/Audio/abc/hls/segment/stream.mp3", headers)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"0123456789");

    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-2"));
    headers.insert(
        header::IF_RANGE,
        HeaderValue::from_static("Wed, 21 Oct 2037 07:28:00 GMT"),
    );
    let response = fixture
        .get("/Audio/abc/hls/segment/stream.mp3", headers)
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(body(response).await, b"012");

    fixture.cleanup().await;
}

#[tokio::test]
async fn official_playlist_rows_use_default_authentication_and_real_file_streaming() {
    let fixture = Fixture::new().await;
    let playlist_body = b"#EXTM3U\n#EXTINF:4,\nseg1.ts\n";
    fs::write(fixture.transcode_path().join("list.m3u8"), playlist_body).unwrap();

    assert_eq!(
        fixture
            .get("/Videos/abc/hls/list/stream.m3u8", HeaderMap::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // Official valid row with an ordinary device user.
    let response = fixture
        .get("/Videos/abc/hls/list/stream.m3u8", fixture.device_headers())
        .await;
    let playlist_mime = MimeTypes::get_mime_type("list.m3u8").unwrap();
    assert_file_response(response, StatusCode::OK, &playlist_mime, playlist_body).await;

    // API keys satisfy the same default policy and query-token extraction.
    let uri = format!(
        "/Videos/abc/hls/list/stream.m3u8?api_key={}",
        fixture.api_key_token
    );
    assert_eq!(
        fixture.get(&uri, HeaderMap::new()).await.status(),
        StatusCode::OK
    );

    // Official non-m3u8 and traversal rows remain playlist requests.
    assert_eq!(
        fixture
            .get("/Videos/abc/hls/list/stream.mp4", fixture.device_headers(),)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .get(
                "/Videos/abc/hls/%2E%2E%2F%2E%2E%2F%2E%2E%2F%2E%2E%2Fetc%2Fpasswd/stream.m3u8",
                fixture.device_headers(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let mut headers = fixture.device_headers();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-6"));
    let response = fixture
        .get("/Videos/abc/hls/list/stream.m3u8", headers)
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_TYPE], playlist_mime);
    assert_eq!(body(response).await, b"#EXTM3U");

    assert_eq!(
        fixture
            .get(
                "/Videos/abc/hls/missing/stream.m3u8",
                fixture.device_headers(),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.block_ordinary_user().await;
    assert_eq!(
        fixture
            .get("/Videos/abc/hls/list/stream.m3u8", fixture.device_headers(),)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn official_video_rows_validate_before_scan_and_gate_real_segments() {
    let fixture = Fixture::new().await;
    fs::write(fixture.transcode_path().join("seg1.ts"), b"video-segment").unwrap();
    fs::write(
        fixture.transcode_path().join("PREFIX-PlayList123.M3U8"),
        b"#EXTM3U",
    )
    .unwrap();

    // Official valid row: anonymous, case-insensitive playlist basename match.
    let response = fixture
        .get(
            "/Videos/arbitrary-item/hls/playlist123/seg1.ts",
            HeaderMap::new(),
        )
        .await;
    assert_file_response(response, StatusCode::OK, "video/mp2t", b"video-segment").await;

    // Official no-matching-playlist row.
    assert_eq!(
        fixture
            .get("/Videos/abc/hls/not-present/seg1.ts", HeaderMap::new(),)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Official traversal row is rejected before any directory scan.
    assert_eq!(
        fixture
            .get(
                "/Videos/abc/hls/playlist123/%2E%2E%2F%2E%2E%2F%2E%2E%2F%2E%2E%2Fetc%2Fpasswd.ts",
                HeaderMap::new(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    // A matching active playlist is distinct from the requested physical segment.
    assert_eq!(
        fixture
            .get("/Videos/abc/hls/playlist123/missing.ts", HeaderMap::new(),)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Playlist discovery is non-recursive and ignores matching directories.
    fs::create_dir(fixture.transcode_path().join("nested")).unwrap();
    fs::write(
        fixture.transcode_path().join("nested/deep-playlist.m3u8"),
        b"#EXTM3U",
    )
    .unwrap();
    fs::create_dir(fixture.transcode_path().join("directory-playlist.ts")).unwrap();
    for playlist_id in ["deep-playlist", "directory-playlist"] {
        let uri = format!("/Videos/abc/hls/{playlist_id}/seg1.ts");
        assert_eq!(
            fixture.get(&uri, HeaderMap::new()).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=6-12"));
    let response = fixture
        .get("/Videos/abc/hls/playlist123/seg1.ts", headers)
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp2t");
    assert_eq!(body(response).await, b"segment");

    // With no physical transcode directory, validation still wins over scan/missing-file logic.
    let missing_root_app = jellyfin_api::router(
        AppState::new(
            fixture.database.clone(),
            "HLS Missing Root".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_transcode_directory(fixture.temporary.path().join("does-not-exist")),
    );
    assert_eq!(
        raw_get(
            &missing_root_app,
            "/Videos/abc/hls/playlist123/%2E%2E%2Foutside.ts",
            HeaderMap::new(),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw_get(
            &missing_root_app,
            "/Videos/abc/hls/playlist123/seg1.ts",
            HeaderMap::new(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn dynamic_hls_routes_require_auth_and_stream_generated_files() {
    let fixture = Fixture::new().await;
    let item_id = Uuid::new_v4();
    let items = BaseItemRepository::new(fixture.database.clone());
    let mut item = NewBaseItem::new(item_id, "Movie");
    item.path = Some("/media/dynamic-hls.mkv".to_owned());
    items.create(item).await.expect("dynamic HLS item");
    fs::write(
        fixture.transcode_path().join("master.m3u8"),
        b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nmain.m3u8\n",
    )
    .unwrap();
    fs::write(
        fixture.transcode_path().join("main.m3u8"),
        b"#EXTM3U\n#EXTINF:4,\nhls1/main/0.ts\n",
    )
    .unwrap();
    fs::write(
        fixture.transcode_path().join("live.m3u8"),
        b"#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n",
    )
    .unwrap();
    fs::write(fixture.transcode_path().join("main0.ts"), b"dynamic-video").unwrap();
    fs::write(
        fixture.transcode_path().join("audio0.aac"),
        b"dynamic-audio",
    )
    .unwrap();

    for uri in [
        format!("/Videos/{item_id}/master.m3u8"),
        format!("/Videos/{item_id}/main.m3u8"),
        format!("/Videos/{item_id}/live.m3u8"),
        format!("/Audio/{item_id}/master.m3u8"),
        format!("/Audio/{item_id}/main.m3u8"),
        format!("/videos/{item_id}/master.m3u8"),
        format!("/videos/{item_id}/main.m3u8"),
        format!("/videos/{item_id}/live.m3u8"),
        format!("/audio/{item_id}/master.m3u8"),
        format!("/audio/{item_id}/main.m3u8"),
        format!(
            "/Videos/{item_id}/hls1/main/0.ts?runtimeTicks=0&actualSegmentLengthTicks=40000000"
        ),
        format!(
            "/Audio/{item_id}/hls1/audio/0.aac?runtimeTicks=0&actualSegmentLengthTicks=40000000"
        ),
    ] {
        assert_eq!(
            fixture.get(&uri, HeaderMap::new()).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }

    let response = fixture
        .get(
            &format!("/Videos/{item_id}/master.m3u8"),
            fixture.device_headers(),
        )
        .await;
    let playlist_mime = MimeTypes::get_mime_type("playlist.m3u8").unwrap();
    assert_file_response(
        response,
        StatusCode::OK,
        &playlist_mime,
        b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nmain.m3u8\n",
    )
    .await;

    let response = fixture
        .get(
            &format!("/videos/{item_id}/master.m3u8"),
            fixture.device_headers(),
        )
        .await;
    assert_file_response(
        response,
        StatusCode::OK,
        &playlist_mime,
        b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nmain.m3u8\n",
    )
    .await;

    let mut headers = fixture.device_headers();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-6"));
    let response = fixture
        .get(&format!("/Videos/{item_id}/main.m3u8"), headers)
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_TYPE], playlist_mime);
    assert_eq!(body(response).await, b"#EXTM3U");

    let response = fixture
        .get(
            &format!(
                "/Videos/{item_id}/hls1/main/0.ts?runtimeTicks=0&actualSegmentLengthTicks=40000000"
            ),
            fixture.device_headers(),
        )
        .await;
    assert_file_response(response, StatusCode::OK, "video/mp2t", b"dynamic-video").await;

    let response = fixture
        .get(
            &format!(
                "/Audio/{item_id}/hls1/audio/0.aac?runtimeTicks=0&actualSegmentLengthTicks=40000000"
            ),
            fixture.device_headers(),
        )
        .await;
    assert_file_response(response, StatusCode::OK, "audio/aac", b"dynamic-audio").await;

    for query in [
        "RuntimeTicks=0&ActualSegmentLengthTicks=40000000",
        "runtimeticks=0&actualsegmentlengthticks=40000000",
    ] {
        let response = fixture
            .get(
                &format!("/Videos/{item_id}/hls1/main/0.ts?{query}"),
                fixture.device_headers(),
            )
            .await;
        assert_file_response(response, StatusCode::OK, "video/mp2t", b"dynamic-video").await;
    }

    let response = fixture
        .get(
            &format!(
                "/videos/{item_id}/hls1/main/0.ts?runtimeTicks=0&actualSegmentLengthTicks=40000000"
            ),
            fixture.device_headers(),
        )
        .await;
    assert_file_response(response, StatusCode::OK, "video/mp2t", b"dynamic-video").await;

    let response = fixture
        .get(
            &format!(
                "/audio/{item_id}/hls1/audio/0.aac?runtimeTicks=0&actualSegmentLengthTicks=40000000"
            ),
            fixture.device_headers(),
        )
        .await;
    assert_file_response(response, StatusCode::OK, "audio/aac", b"dynamic-audio").await;

    for uri in [
        format!("/Videos/{item_id}/hls1/main/0.ts?actualSegmentLengthTicks=40000000"),
        format!("/Videos/{item_id}/hls1/main/0.ts?runtimeTicks=0"),
        format!("/Videos/{item_id}/hls1/main/0.ts?runtimeTicks=0&actualSegmentLengthTicks=0"),
        format!(
            "/Videos/{item_id}/hls1/main/0.ts?runtimeTicks=0&actualSegmentLengthTicks=40000000&startTimeTicks=1"
        ),
        format!(
            "/Videos/{item_id}/hls1/%2E%2E%2Foutside/0.ts?runtimeTicks=0&actualSegmentLengthTicks=40000000"
        ),
    ] {
        assert_eq!(
            fixture.get(&uri, fixture.device_headers()).await.status(),
            StatusCode::BAD_REQUEST,
            "{uri}"
        );
    }

    fixture.block_ordinary_user().await;
    assert_eq!(
        fixture
            .get(
                &format!("/Videos/{item_id}/master.m3u8"),
                fixture.device_headers()
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    items
        .delete(item_id)
        .await
        .expect("dynamic HLS item cleanup");

    fixture.cleanup().await;
}

#[tokio::test]
async fn unknown_alternate_runtime_uses_event_hls_while_known_runtimes_remain_vod() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());

    let primary_id = Uuid::new_v4();
    let mut primary = NewBaseItem::new(primary_id, "Movie");
    primary.path = Some("/media/hls-known-primary.mkv".to_owned());
    primary.runtime_ticks = Some(120_000_000);
    items.create(primary).await.expect("known primary item");

    let alternate_id = Uuid::new_v4();
    let mut alternate = NewBaseItem::new(alternate_id, "Movie");
    alternate.path = Some("/media/hls-unknown-alternate.mkv".to_owned());
    alternate.primary_version_id = Some(primary_id);
    items
        .create(alternate)
        .await
        .expect("unknown runtime alternate");

    let response = fixture
        .get(
            &format!(
                "/Videos/{primary_id}/master.m3u8?videoCodec=h264&mediasourceid={alternate_id}"
            ),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let master = String::from_utf8(body(response).await).expect("UTF-8 master playlist");
    let live_url = master
        .lines()
        .find(|line| line.starts_with("live.m3u8?"))
        .expect("unknown runtime master must select the live playlist");
    assert!(live_url.contains(&format!("mediasourceid={alternate_id}")));
    let response = fixture
        .get(
            &format!("/Videos/{primary_id}/{live_url}"),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let event = String::from_utf8(body(response).await).expect("UTF-8 event playlist");
    assert!(event.contains("#EXT-X-PLAYLIST-TYPE:EVENT"));
    let segment_url = event
        .lines()
        .find(|line| line.starts_with("hls/"))
        .expect("event playlist segment URL");
    let response = fixture
        .get(
            &format!("/Videos/{primary_id}/{segment_url}"),
            fixture.device_headers(),
        )
        .await;
    assert_file_response(
        response,
        StatusCode::OK,
        "video/mp2t",
        b"hls-unknown-alternate.mkv",
    )
    .await;

    let zero_id = Uuid::new_v4();
    let mut zero = NewBaseItem::new(zero_id, "Movie");
    zero.path = Some("/media/hls-zero.mkv".to_owned());
    zero.runtime_ticks = Some(0);
    items.create(zero).await.expect("zero runtime item");
    let response = fixture
        .get(
            &format!("/Videos/{zero_id}/master.m3u8?videoCodec=h264"),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let response = fixture
        .get(
            &format!("/Videos/{primary_id}/master.m3u8?videoCodec=h264"),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let master = String::from_utf8(body(response).await).expect("UTF-8 master playlist");
    let main_url = master
        .lines()
        .find(|line| line.starts_with("main.m3u8?"))
        .expect("known runtime master must select the VOD playlist");
    let response = fixture
        .get(
            &format!("/Videos/{primary_id}/{main_url}"),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let vod = String::from_utf8(body(response).await).expect("UTF-8 VOD playlist");
    assert!(vod.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
    assert!(vod.contains("#EXT-X-ENDLIST"));

    for item_id in [alternate_id, primary_id, zero_id] {
        items.delete(item_id).await.expect("HLS item cleanup");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn universal_audio_hls_uses_the_authenticated_audio_playlist_pipeline() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let item_id = Uuid::new_v4();
    let mut item = NewBaseItem::new(item_id, "Audio");
    item.path = Some("/media/universal-audio.mp3".to_owned());
    item.runtime_ticks = Some(120_000_000);
    items.create(item).await.expect("universal audio item");

    let response = fixture
        .get(
            &format!(
                "/Audio/{item_id}/universal?transcodingProtocol=hls&audioCodec=aac&audioBitRate=128000&maxAudioChannels=2"
            ),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let master = String::from_utf8(body(response).await).expect("UTF-8 master playlist");
    let main_url = master
        .lines()
        .find(|line| line.starts_with("main.m3u8?"))
        .expect("Universal Audio HLS master must select the main playlist");
    assert!(main_url.contains("audioBitrate=128000"));
    assert!(main_url.contains("transcodingMaxAudioChannels=2"));

    let response = fixture
        .get(
            &format!("/Audio/{item_id}/{main_url}"),
            fixture.device_headers(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let playlist = String::from_utf8(body(response).await).expect("UTF-8 VOD playlist");
    assert!(playlist.contains("#EXT-X-PLAYLIST-TYPE:VOD"));
    let segment_url = playlist
        .lines()
        .find(|line| line.starts_with("/Audio/"))
        .expect("Universal Audio HLS VOD segment URL");
    assert!(
        segment_url.contains(&format!("userId={}", fixture.user_id)),
        "HLS segments must retain the playback target user"
    );
    let response = fixture.get(segment_url, fixture.device_headers()).await;
    assert_file_response(
        response,
        StatusCode::OK,
        "video/mp2t",
        b"universal-audio.mp3",
    )
    .await;

    items
        .delete(item_id)
        .await
        .expect("Universal Audio item cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn dynamic_hls_rejects_media_sources_outside_the_requested_version_group() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let item_id = Uuid::new_v4();
    let mut item = NewBaseItem::new(item_id, "Movie");
    item.path = Some("/media/hls-primary.mkv".to_owned());
    items.create(item).await.expect("HLS primary item");
    let unrelated_id = Uuid::new_v4();
    let mut unrelated = NewBaseItem::new(unrelated_id, "Movie");
    unrelated.path = Some("/media/hls-unrelated.mkv".to_owned());
    items.create(unrelated).await.expect("unrelated HLS item");

    for query_name in ["MediaSourceId", "mediaSourceId", "mediasourceid"] {
        let response = fixture
            .get(
                &format!(
                    "/Videos/{item_id}/master.m3u8?videoCodec=h264&{query_name}={unrelated_id}"
                ),
                fixture.device_headers(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{query_name}");
    }

    items.delete(unrelated_id).await.expect("unrelated cleanup");
    items.delete(item_id).await.expect("primary cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn active_encoding_cleanup_matches_official_auth_and_required_query_contract() {
    let fixture = Fixture::new().await;
    let route = "/Videos/ActiveEncodings?deviceId=hls-tests&playSessionId=play-session";

    assert_eq!(
        fixture.delete(route, HeaderMap::new()).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .delete(
                "/Videos/ActiveEncodings?deviceId=hls-tests",
                fixture.device_headers()
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .delete(
                "/Videos/ActiveEncodings?deviceId=%20&playSessionId=play-session",
                fixture.device_headers(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    for route in [
        route,
        "/Videos/ActiveEncodings?DeviceId=hls-tests&PlaySessionId=play-session",
        "/Videos/ActiveEncodings?deviceid=hls-tests&playsessionid=play-session",
        "/videos/activeencodings?deviceid=hls-tests&playsessionid=play-session",
    ] {
        assert_eq!(
            fixture
                .delete(route, fixture.device_headers())
                .await
                .status(),
            StatusCode::NO_CONTENT,
            "{route}"
        );
    }

    fixture.block_ordinary_user().await;
    assert_eq!(
        fixture
            .delete(route, fixture.device_headers())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    temporary: TempDirectory,
    user_id: Uuid,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
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
        let user = UserService::new(database.clone())
            .create(&format!("hls-user-{suffix}"))
            .await
            .expect("ordinary HLS user creation");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "HLS Tests",
                "1.0",
                "Test",
                format!("hls-{suffix}"),
            ))
            .await
            .expect("HLS device session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("hls-key-{suffix}"))
            .await
            .expect("HLS API key creation");
        let temporary = TempDirectory::new();
        let transcode_path = temporary.path().join(TRANSCODE_DIRECTORY_NAME);
        fs::create_dir(&transcode_path).unwrap();
        let fake_ffmpeg = temporary.path().join("fake-ffmpeg");
        fs::write(
            &fake_ffmpeg,
            b"#!/bin/sh\nsegment_pattern=\nbase_url=\ninput_path=\nplaylist=\nexpect=\nfor argument in \"$@\"; do\n  if [ \"$expect\" = segment ]; then\n    segment_pattern=$argument\n    expect=\n  elif [ \"$expect\" = base ]; then\n    base_url=$argument\n    expect=\n  elif [ \"$expect\" = input ]; then\n    input_path=$argument\n    expect=\n  elif [ \"$argument\" = -hls_segment_filename ]; then\n    expect=segment\n  elif [ \"$argument\" = -hls_base_url ]; then\n    expect=base\n  elif [ \"$argument\" = -i ]; then\n    expect=input\n  fi\n  playlist=$argument\ndone\nsegment_path=$(printf '%s\\n' \"$segment_pattern\" | sed 's/%d/0/')\ninput_name=${input_path##*/}\nprintf '%s' \"$input_name\" > \"$segment_path\"\nsegment_name=${segment_path##*/}\nprintf '#EXTM3U\\n#EXT-X-PLAYLIST-TYPE:EVENT\\n#EXTINF:1,\\n%s%s\\n' \"$base_url\" \"$segment_name\" > \"$playlist\"\n",
        )
        .expect("fake ffmpeg");
        fs::set_permissions(&fake_ffmpeg, fs::Permissions::from_mode(0o700))
            .expect("fake ffmpeg permissions");
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "HLS Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_transcode_directory(transcode_path)
            .with_ffmpeg_path(fake_ffmpeg),
        );
        Self {
            database,
            app,
            temporary,
            user_id: user.id,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
        }
    }

    fn transcode_path(&self) -> PathBuf {
        self.temporary.path().join(TRANSCODE_DIRECTORY_NAME)
    }

    fn device_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("{AUTHORIZATION}, Token=\"{}\"", self.user_token))
                .unwrap(),
        );
        headers
    }

    async fn get(&self, uri: &str, headers: HeaderMap) -> axum::response::Response {
        raw_get(&self.app, uri, headers).await
    }

    async fn delete(&self, uri: &str, headers: HeaderMap) -> axum::response::Response {
        raw_request(&self.app, "DELETE", uri, headers).await
    }

    async fn block_ordinary_user(&self) {
        let policy = UserPolicy {
            access_schedules: vec![AccessSchedule {
                day_of_week: DynamicDayOfWeek::Everyday,
                start_hour: 18.0,
                end_hour: 6.0,
            }],
            authentication_provider_id: Some(
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
            ),
            password_reset_provider_id: Some(
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
            ),
            ..UserPolicy::default()
        };
        UserService::new(self.database.clone())
            .update_policy(self.user_id, &policy)
            .await
            .expect("blocked HLS user policy update");
    }

    async fn cleanup(self) {
        api_key::Entity::delete_many()
            .filter(api_key::Column::Id.eq(self.api_key_id))
            .exec(&self.database)
            .await
            .expect("HLS API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("HLS user cleanup");
    }
}

async fn raw_get(app: &axum::Router, uri: &str, headers: HeaderMap) -> axum::response::Response {
    raw_request(app, "GET", uri, headers).await
}

async fn raw_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: HeaderMap,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    *request.headers_mut() = headers;
    app.clone().oneshot(request).await.unwrap()
}

async fn assert_file_response(
    response: axum::response::Response,
    status: StatusCode,
    mime_type: &str,
    expected_body: &[u8],
) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], mime_type);
    assert_eq!(body(response).await, expected_body);
}

async fn body(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("HLS response body")
        .to_vec()
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jellyfin-rust-hls-api-{}", Uuid::new_v4().simple()));
        fs::create_dir(&path).expect("HLS temporary directory creation");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
