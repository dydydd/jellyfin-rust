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
use jellyfin_model::{MediaStream, MediaStreamType, UserPolicy};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path};
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
async fn playback_info_exposes_and_selects_grouped_video_versions() {
    let fixture = Fixture::new().await;
    let alternate_ids = [Uuid::new_v4(), Uuid::new_v4()];
    let items = BaseItemRepository::new(fixture.database.clone());
    let streams = MediaStreamService::new(fixture.database.clone());
    streams
        .save_media_streams(fixture.item_id, version_streams("h264"))
        .await
        .expect("primary version media streams");
    for alternate_id in alternate_ids {
        let mut alternate = NewBaseItem::new(alternate_id, "Movie");
        alternate.name = Some("playback-info-movie".to_owned());
        alternate.path = Some(format!("/media/playback-info-alternate-{alternate_id}.mkv"));
        alternate.primary_version_id = Some(fixture.item_id);
        alternate.runtime_ticks = Some(12_345_000_000);
        items
            .create(alternate)
            .await
            .expect("alternate playback item");
        streams
            .save_media_streams(alternate_id, version_streams("h264"))
            .await
            .expect("alternate media stream");
    }

    let route = format!("/Items/{}/PlaybackInfo", fixture.item_id);
    let all_sources = body_json(fixture.get(&route, Some(&fixture.user_token)).await).await;
    let sources = all_sources["MediaSources"]
        .as_array()
        .expect("media sources");
    assert_eq!(sources.len(), 3);
    assert_eq!(sources[0]["Id"], fixture.item_id.simple().to_string());
    assert_eq!(sources[0]["Bitrate"], 5_500_000);
    let source_order = sources
        .iter()
        .map(|source| source["Id"].as_str().expect("media source id").to_owned())
        .collect::<Vec<_>>();
    for source in sources {
        assert_version_stream_language_shape(source);
    }

    let detail_route = format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_id);
    let detail = body_json(fixture.get(&detail_route, Some(&fixture.user_token)).await).await;
    assert_eq!(detail["MediaSources"].as_array().unwrap().len(), 3);
    for source in detail["MediaSources"].as_array().unwrap() {
        assert_version_stream_language_shape(source);
    }
    assert_version_stream_language_shape(&json!({ "MediaStreams": detail["MediaStreams"] }));

    let transcode_id = Uuid::parse_str(&source_order[1]).expect("first alternate id");
    let direct_alternate_id = Uuid::parse_str(&source_order[2]).expect("second alternate id");
    streams
        .save_media_streams(transcode_id, version_streams("hevc"))
        .await
        .expect("incompatible alternate stream");

    let profiled = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({
                    // Official Jellyfin only applies an explicit stream index
                    // when the request also selects its MediaSourceId.
                    "AudioStreamIndex": 99,
                    "MaxStreamingBitrate": 100_000_000,
                    "DeviceProfile": flexible_video_profile(true)
                })),
            )
            .await,
    )
    .await;
    let profiled_sources = profiled["MediaSources"].as_array().unwrap();
    assert_eq!(
        profiled_sources
            .iter()
            .map(|source| source["Id"].as_str().expect("profiled source id"))
            .collect::<Vec<_>>(),
        source_order.iter().map(String::as_str).collect::<Vec<_>>(),
        "profiling each version must not reorder media sources"
    );
    let primary = profiled_sources
        .iter()
        .find(|source| source["Id"] == fixture.item_id.simple().to_string())
        .expect("profiled primary source");
    assert_eq!(primary["SupportsDirectPlay"], true, "{profiled}");
    assert_eq!(primary["SupportsDirectStream"], false, "{profiled}");
    assert_eq!(primary["SupportsTranscoding"], false, "{profiled}");
    assert!(primary.get("TranscodingUrl").is_none());
    let alternate = profiled_sources
        .iter()
        .find(|source| source["Id"] == transcode_id.simple().to_string())
        .expect("profiled alternate source");
    assert_eq!(alternate["SupportsDirectPlay"], false);
    assert_eq!(alternate["SupportsDirectStream"], false);
    assert_eq!(alternate["SupportsTranscoding"], true);
    let alternate_url = alternate["TranscodingUrl"]
        .as_str()
        .expect("alternate HLS URL");
    assert!(alternate_url.contains("/master.m3u8"), "{alternate_url}");
    assert!(
        alternate_url.contains(&format!("MediaSourceId={}", transcode_id.simple())),
        "{alternate_url}"
    );
    assert!(
        !alternate_url.contains("AudioStreamIndex=99"),
        "{alternate_url}"
    );
    let direct_alternate = profiled_sources
        .iter()
        .find(|source| source["Id"] == direct_alternate_id.simple().to_string())
        .expect("direct-play alternate source");
    assert_eq!(direct_alternate["SupportsDirectPlay"], true);
    assert_eq!(direct_alternate["SupportsDirectStream"], false);
    assert_eq!(direct_alternate["SupportsTranscoding"], false);
    assert!(direct_alternate.get("TranscodingUrl").is_none());

    let selected = body_json(
        fixture
            .post(
                &format!(
                    "{route}?MediaSourceId={}",
                    transcode_id.to_string().to_ascii_uppercase()
                ),
                Some(&fixture.user_token),
                Some(&json!({
                    "AudioStreamIndex": 1,
                    "DeviceProfile": flexible_video_profile(false)
                })),
            )
            .await,
    )
    .await;
    assert_eq!(selected["MediaSources"].as_array().unwrap().len(), 1);
    assert_eq!(
        selected["MediaSources"][0]["Id"],
        transcode_id.simple().to_string()
    );
    assert_eq!(
        selected["MediaSources"][0]["Path"],
        format!("/media/playback-info-alternate-{transcode_id}.mkv")
    );
    let selected_url = selected["MediaSources"][0]["TranscodingUrl"]
        .as_str()
        .expect("selected alternate HLS URL");
    assert!(
        selected_url.contains("AudioStreamIndex=1"),
        "{selected_url}"
    );

    for alternate_id in alternate_ids {
        base_item::Entity::delete_by_id(alternate_id)
            .exec(&fixture.database)
            .await
            .expect("alternate cleanup");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_capabilities_require_an_implemented_method_and_user_policy() {
    let fixture = Fixture::new().await;
    let route = format!("/Items/{}/PlaybackInfo", fixture.item_id);

    let direct = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({ "DeviceProfile": flexible_video_profile(true) })),
            )
            .await,
    )
    .await;
    let direct = &direct["MediaSources"][0];
    assert_eq!(direct["SupportsDirectPlay"], true);
    assert_eq!(direct["SupportsDirectStream"], false);
    assert_eq!(direct["SupportsTranscoding"], false);
    assert!(direct.get("TranscodingUrl").is_none());

    let incompatible = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({
                    "DeviceProfile": {
                        "Name": "Incompatible",
                        "DirectPlayProfiles": [{
                            "Container": "avi",
                            "VideoCodec": "vp9",
                            "AudioCodec": "mp3",
                            "Type": "Video"
                        }],
                        "TranscodingProfiles": []
                    }
                })),
            )
            .await,
    )
    .await;
    assert_no_playback_capabilities(&incompatible["MediaSources"][0]);

    let unimplemented_http_transcode = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({
                    "DeviceProfile": {
                        "Name": "HTTP MP4 Transcode",
                        "DirectPlayProfiles": [],
                        "TranscodingProfiles": [{
                            "Container": "mp4",
                            "Type": "Video",
                            "VideoCodec": "h264",
                            "AudioCodec": "aac",
                            "Protocol": "http",
                            "Context": "Streaming"
                        }]
                    }
                })),
            )
            .await,
    )
    .await;
    assert_no_playback_capabilities(&unimplemented_http_transcode["MediaSources"][0]);

    let transcoded = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({ "DeviceProfile": flexible_video_profile(false) })),
            )
            .await,
    )
    .await;
    let transcoded = &transcoded["MediaSources"][0];
    assert_eq!(transcoded["SupportsDirectPlay"], false);
    assert_eq!(transcoded["SupportsDirectStream"], false);
    assert_eq!(transcoded["SupportsTranscoding"], true);
    assert!(
        transcoded["TranscodingUrl"]
            .as_str()
            .is_some_and(|url| url.contains("/master.m3u8"))
    );

    let users = UserService::new(fixture.database.clone());
    let stored_user = users.get(fixture.user_id).await.expect("playback user");
    let mut policy: UserPolicy =
        serde_json::from_value(stored_user.policy).expect("stored playback policy");
    policy.enable_audio_playback_transcoding = false;
    policy.enable_video_playback_transcoding = false;
    policy.enable_playback_remuxing = false;
    users
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted playback policy");
    let policy_blocked = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({ "DeviceProfile": flexible_video_profile(false) })),
            )
            .await,
    )
    .await;
    assert_no_playback_capabilities(&policy_blocked["MediaSources"][0]);

    fixture.cleanup().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn playback_info_hydrates_only_the_selected_strm_version() {
    let directory = std::env::temp_dir().join(format!(
        "jellyfin-selected-source-probe-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&directory).expect("probe fixture directory");
    let probe_log = directory.join("probe.log");
    let probe_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../jellyfin-media-encoding/tests/fixtures/probing/video_metadata.json");
    let probe_script = directory.join("fake-ffprobe");
    std::fs::write(
        &probe_script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nexec /bin/cat '{}'\n",
            probe_log.display(),
            probe_fixture.display()
        ),
    )
    .expect("fake ffprobe script");
    let mut permissions = std::fs::metadata(&probe_script)
        .expect("fake ffprobe metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&probe_script, permissions).expect("fake ffprobe executable");

    let fixture = Fixture::new_with_ffprobe_path(Some(&probe_script)).await;
    let primary_sidecar = directory.join("Primary.strm");
    let primary_target = directory.join("primary-target.mkv");
    std::fs::write(
        &primary_sidecar,
        primary_target.to_string_lossy().as_bytes(),
    )
    .expect("primary strm sidecar");
    let items = BaseItemRepository::new(fixture.database.clone());
    let mut primary = items
        .get(fixture.item_id)
        .await
        .expect("primary lookup")
        .expect("primary fixture");
    primary.path = Some(primary_sidecar.to_string_lossy().into_owned());
    primary.data = Some(json!({
        "Container": "mkv",
        "StrmTarget": primary_target.to_string_lossy()
    }));
    items.update(primary).await.expect("primary strm update");

    let alternate_id = Uuid::new_v4();
    let alternate_sidecar = directory.join("Alternate.strm");
    let alternate_target = directory.join("alternate-target.mkv");
    std::fs::write(
        &alternate_sidecar,
        alternate_target.to_string_lossy().as_bytes(),
    )
    .expect("alternate strm sidecar");
    let mut alternate = NewBaseItem::new(alternate_id, "Movie");
    alternate.name = Some("playback-info-movie".to_owned());
    alternate.path = Some(alternate_sidecar.to_string_lossy().into_owned());
    alternate.primary_version_id = Some(fixture.item_id);
    alternate.data = Some(json!({
        "Container": "mkv",
        "StrmTarget": alternate_target.to_string_lossy()
    }));
    items.create(alternate).await.expect("alternate strm item");

    let streams = MediaStreamService::new(fixture.database.clone());
    for item_id in [fixture.item_id, alternate_id] {
        streams
            .save_media_streams(
                item_id,
                vec![MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    is_default: true,
                    ..MediaStream::default()
                }],
            )
            .await
            .expect("strm placeholder stream");
    }

    let route = format!(
        "/Items/{}/PlaybackInfo?mediasourceid={}",
        fixture.item_id,
        alternate_id.simple().to_string().to_ascii_uppercase()
    );
    let playback = body_json(
        fixture
            .post(
                &route,
                Some(&fixture.user_token),
                Some(&json!({ "DeviceProfile": flexible_video_profile(false) })),
            )
            .await,
    )
    .await;
    assert_eq!(playback["MediaSources"].as_array().unwrap().len(), 1);
    let selected = &playback["MediaSources"][0];
    assert_eq!(selected["Id"], alternate_id.simple().to_string());
    assert_eq!(selected["MediaStreams"][0]["Codec"], "h264");
    assert_eq!(selected["MediaStreams"][1]["Codec"], "eac3");
    assert_eq!(selected["SupportsDirectPlay"], false);
    assert!(
        selected["TranscodingUrl"]
            .as_str()
            .is_some_and(|url| url.contains(&format!("MediaSourceId={}", alternate_id.simple())))
    );

    let primary_streams = streams
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("primary placeholder lookup");
    assert_eq!(primary_streams.len(), 1);
    assert_eq!(primary_streams[0].codec, None);
    let probe_arguments = std::fs::read_to_string(&probe_log).expect("probe invocation log");
    assert!(
        probe_arguments.contains(alternate_target.to_string_lossy().as_ref()),
        "selected alternate must be probed"
    );
    assert!(
        !probe_arguments.contains(primary_target.to_string_lossy().as_ref()),
        "the displayed primary must not be probed"
    );

    let camel_case = body_json(
        fixture
            .post(
                &format!("/Items/{}/PlaybackInfo", fixture.item_id),
                Some(&fixture.user_token),
                Some(&json!({ "mediaSourceId": alternate_id.simple().to_string() })),
            )
            .await,
    )
    .await;
    assert_eq!(camel_case["MediaSources"].as_array().unwrap().len(), 1);
    assert_eq!(
        camel_case["MediaSources"][0]["Id"],
        alternate_id.simple().to_string()
    );

    base_item::Entity::delete_by_id(alternate_id)
        .exec(&fixture.database)
        .await
        .expect("alternate cleanup");
    fixture.cleanup().await;
    std::fs::remove_dir_all(directory).expect("probe fixture cleanup");
}

#[tokio::test]
async fn get_playback_info_preserves_media_source_and_bitrate_query_options() {
    let fixture = Fixture::new().await;
    let source_id = fixture.item_id.simple().to_string().to_ascii_uppercase();
    let playback = body_json(
        fixture
            .get(
                &format!(
                    "/Items/{}/PlaybackInfo?MediaSourceId={source_id}&MaxStreamingBitrate=1",
                    fixture.item_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;

    assert_eq!(
        playback["MediaSources"][0]["Id"],
        fixture.item_id.simple().to_string()
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn posted_playback_info_accepts_legacy_casing_numeric_strings_and_query_precedence() {
    let fixture = Fixture::new().await;
    let uppercase_compact_source_id = fixture.item_id.simple().to_string().to_ascii_uppercase();
    let route = format!(
        "/Items/{}/PlaybackInfo?userid={}&maxstreamingbitrate=8000000&starttimeticks=456&audiostreamindex=1&subtitlestreamindex=-1&maxaudiochannels=2&mediasourceid={}&autoopenlivestream=False&enabledirectplay=false&enabledirectstream=True&enabletranscoding=true&allowvideostreamcopy=false&allowaudiostreamcopy=true",
        fixture.item_id, fixture.user_id, uppercase_compact_source_id
    );
    let body = json!({
        "userid": fixture.admin_id,
        "maxstreamingbitrate": "1",
        "starttimeticks": "123",
        "audiostreamindex": "0",
        "subtitlestreamindex": "-1",
        "maxaudiochannels": "1",
        "mediasourceid": "ignored-by-query",
        "enabledirectplay": "true",
        "enabledirectstream": "false",
        "enabletranscoding": "false",
        "allowvideostreamcopy": "true",
        "allowaudiostreamcopy": "false",
        "autoopenlivestream": "true",
        "alwaysburninsubtitlewhentranscoding": "true",
        "deviceprofile": flexible_video_profile(true)
    });

    let response = fixture
        .post(&route, Some(&fixture.user_token), Some(&body))
        .await;
    let playback = body_json(response).await;
    let source = &playback["MediaSources"][0];
    let url = source["TranscodingUrl"]
        .as_str()
        .expect("query must force the matching MKV profile to transcode");
    assert!(url.contains("/master.m3u8"), "{url}");
    assert!(url.contains("AudioStreamIndex=1"), "{url}");
    assert!(!url.contains("Static=true"), "{url}");
    assert_eq!(source["SupportsDirectPlay"], false);
    assert_eq!(source["SupportsDirectStream"], false);
    assert_eq!(source["SupportsTranscoding"], true);
    assert_eq!(source["TranscodingContainer"], "ts");
    assert_eq!(source["TranscodingSubProtocol"], "hls");

    fixture.cleanup().await;
}

#[tokio::test]
async fn playback_info_returns_no_compatible_stream_for_an_empty_source_selection() {
    let fixture = Fixture::new().await;
    let playback = body_json(
        fixture
            .post(
                &format!(
                    "/Items/{}/PlaybackInfo?MediaSourceId=missing-source",
                    fixture.item_id
                ),
                Some(&fixture.user_token),
                None,
            )
            .await,
    )
    .await;

    assert_eq!(playback["MediaSources"], json!([]));
    assert_eq!(playback["ErrorCode"], "NoCompatibleStream");
    assert!(playback.get("PlaySessionId").is_none());

    fixture.cleanup().await;
}

#[tokio::test]
async fn posted_playback_info_uses_current_session_capabilities_profile_as_fallback() {
    let fixture = Fixture::new().await;
    let profile = json!({
        "DeviceProfile": {
            "Name": "Session Capabilities Profile",
            "DirectPlayProfiles": [],
            "TranscodingProfiles": [{
                "Container": "ts",
                "Type": "Video",
                "VideoCodec": "h264",
                "AudioCodec": "aac",
                "Protocol": "hls",
                "Context": "Streaming",
                "SegmentLength": 6
            }]
        }
    });
    let capabilities_response = fixture
        .post(
            "/Sessions/Capabilities/Full",
            Some(&fixture.user_token),
            Some(&profile),
        )
        .await;
    assert_eq!(capabilities_response.status(), StatusCode::NO_CONTENT);

    let playback = body_json(
        fixture
            .post(
                &format!("/Items/{}/PlaybackInfo", fixture.item_id),
                Some(&fixture.user_token),
                None,
            )
            .await,
    )
    .await;
    let source = &playback["MediaSources"][0];
    let url = source["TranscodingUrl"]
        .as_str()
        .expect("stored session profile must select transcoding");
    assert!(url.contains("/master.m3u8"), "{url}");
    assert!(url.contains("SegmentLength=6"), "{url}");
    assert_eq!(source["SupportsDirectPlay"], false);
    assert_eq!(source["SupportsDirectStream"], false);
    assert_eq!(source["SupportsTranscoding"], true);
    assert_eq!(source["TranscodingContainer"], "ts");
    assert_eq!(source["TranscodingSubProtocol"], "hls");

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
    assert_no_playback_capabilities(source);
    assert_eq!(source["MediaStreams"][0]["Index"], 0);
    assert_eq!(source["MediaStreams"][0]["Type"], "Video");
    assert_eq!(source["MediaStreams"][0]["Codec"], "h264");
    assert_eq!(source["MediaStreams"][0]["Width"], 1920);
    assert_eq!(source["MediaStreams"][0]["Height"], 1080);
    assert_eq!(source["MediaStreams"][1]["Index"], 1);
    assert_eq!(source["MediaStreams"][1]["Type"], "Audio");
    assert_eq!(source["MediaStreams"][1]["Codec"], "aac");
    assert_eq!(source["MediaStreams"][1]["Language"], "eng");
    assert_eq!(
        source["MediaStreams"][1]["DisplayTitle"],
        "English - AAC - 2 ch - Default"
    );
}

fn assert_no_playback_capabilities(source: &Value) {
    assert_eq!(source["SupportsDirectPlay"], false);
    assert_eq!(source["SupportsDirectStream"], false);
    assert_eq!(source["SupportsTranscoding"], false);
    assert!(source.get("TranscodingUrl").is_none());
}

fn flexible_video_profile(include_direct_play: bool) -> Value {
    let direct_play_profiles = if include_direct_play {
        json!([{
            "container": "mkv",
            "audioCodec": "aac",
            "videoCodec": "h264",
            "type": "video"
        }])
    } else {
        json!([])
    };
    json!({
        "name": "Legacy Flexible Profile",
        "maxStreamingBitrate": "8000000",
        "directPlayProfiles": direct_play_profiles,
        "transcodingProfiles": [{
            "container": "ts",
            "type": 1,
            "videoCodec": "h264",
            "audioCodec": "aac",
            "protocol": "HLS",
            "context": "streaming",
            "transcodeSeekInfo": "1",
            "minSegments": "2",
            "segmentLength": "6"
        }]
    })
}

fn version_streams(video_codec: &str) -> Vec<MediaStream> {
    vec![
        MediaStream {
            index: 0,
            stream_type: MediaStreamType::Video,
            codec: Some(video_codec.to_owned()),
            bit_rate: Some(4_000_000),
            ..MediaStream::default()
        },
        MediaStream {
            index: 1,
            stream_type: MediaStreamType::Audio,
            codec: Some("aac".to_owned()),
            language: Some("ger".to_owned()),
            channels: Some(2),
            bit_rate: Some(192_000),
            is_default: true,
            ..MediaStream::default()
        },
        MediaStream {
            index: 2,
            stream_type: MediaStreamType::Subtitle,
            codec: Some("srt".to_owned()),
            language: Some("qaa".to_owned()),
            ..MediaStream::default()
        },
    ]
}

fn assert_version_stream_language_shape(source: &Value) {
    let streams = source["MediaStreams"].as_array().expect("media streams");
    let audio = streams
        .iter()
        .find(|stream| stream["Type"] == "Audio")
        .expect("audio stream");
    assert_eq!(audio["Index"], 1);
    assert_eq!(audio["Language"], "deu");
    assert_eq!(audio["LocalizedLanguage"], "German");
    assert_eq!(audio["DisplayTitle"], "German - AAC - 2 ch - Default");
    assert_eq!(audio["BitRate"], 192_000);

    let subtitle = streams
        .iter()
        .find(|stream| stream["Type"] == "Subtitle")
        .expect("subtitle stream");
    assert_eq!(subtitle["Index"], 2);
    assert_eq!(subtitle["Language"], "qaa");
    assert!(subtitle.get("LocalizedLanguage").is_none());
    assert_eq!(subtitle["DisplayTitle"], "Qaa - SRT");
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
        Self::new_with_ffprobe_path(None).await
    }

    async fn new_with_ffprobe_path(ffprobe_path: Option<&Path>) -> Self {
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
        item.data = Some(json!({"Bitrate": 5_500_000}));
        BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("playback info item creation");
        MediaStreamService::new(database.clone())
            .save_media_streams(
                item_id,
                vec![
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
        let mut state = AppState::new(
            database.clone(),
            "Media Info Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        if let Some(ffprobe_path) = ffprobe_path {
            state = state.with_ffprobe_path(ffprobe_path);
        }
        let app = jellyfin_api::router(state);
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
