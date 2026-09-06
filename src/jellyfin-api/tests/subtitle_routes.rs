#![allow(clippy::too_many_lines)]
use std::path::PathBuf;

use axum::{
    body::{Body, Bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamFilter, MediaStreamService, UserService};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice,
    entities::{item_value, user},
};
use jellyfin_model::{MediaStream, MediaStreamType, UserPolicy};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Subtitle Tests\", Device=\"Test\", DeviceId=\"subtitle-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_subtitle_routes_";

#[tokio::test]
async fn delete_subtitle_route_uses_subtitle_management_and_deletes_only_target_stream() {
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
        exercise_delete_subtitle_route(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_delete_subtitle_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = Fixture::subtitle_route(fixture.item_id, 2);

    assert_eq!(
        fixture.send(Method::DELETE, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::subtitle_route(Uuid::new_v4(), 2),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.manager_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.manager_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("media streams after delete");
    let remaining = streams
        .iter()
        .map(|stream| (stream.index, stream.stream_type))
        .collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec![
            (0, MediaStreamType::Video),
            (1, MediaStreamType::Audio),
            (3, MediaStreamType::Subtitle),
        ]
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn remote_subtitle_routes_match_management_policy_and_empty_provider_contract() {
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
        exercise_remote_subtitle_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[tokio::test]
async fn upload_subtitle_route_decodes_base64_file_and_persists_external_stream() {
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
        exercise_upload_subtitle_route(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_remote_subtitle_routes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let search_route = Fixture::search_route(fixture.item_id, "eng");

    assert_eq!(
        fixture
            .send(Method::GET, &search_route, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::GET, &search_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let search = fixture
        .send(Method::GET, &search_route, Some(&fixture.manager_token))
        .await;
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(body_json(search).await, Value::Array(Vec::new()));

    let missing_search = fixture
        .send(
            Method::GET,
            &Fixture::search_route(Uuid::new_v4(), "eng"),
            Some(&fixture.manager_token),
        )
        .await;
    assert_eq!(missing_search.status(), StatusCode::NOT_FOUND);

    let non_video_search = fixture
        .send(
            Method::GET,
            &Fixture::search_route(fixture.folder_id, "eng"),
            Some(&fixture.manager_token),
        )
        .await;
    assert_eq!(non_video_search.status(), StatusCode::NOT_FOUND);

    let download_route = Fixture::download_route(fixture.item_id, "provider-subtitle-id");
    assert_eq!(
        fixture
            .send(Method::POST, &download_route, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::POST, &download_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(Method::POST, &download_route, Some(&fixture.manager_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &Fixture::download_route(Uuid::new_v4(), "provider-subtitle-id"),
                Some(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let provider_route = "/Providers/Subtitles/Subtitles/provider-subtitle-id";
    assert_eq!(
        fixture
            .send(Method::GET, provider_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(Method::GET, provider_route, Some(&fixture.manager_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

async fn exercise_upload_subtitle_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = Fixture::upload_route(fixture.item_id);
    let body = json!({
        "Language": "Eng",
        "Format": "SRT",
        "IsForced": true,
        "IsHearingImpaired": false,
        "Data": "MQowMDowMDowMSwwMDAgLS0+IDAwOjAwOjAyLDAwMApIZWxsbyBmcm9tIHVwbG9hZAo="
    });

    assert_eq!(
        fixture
            .send_json(Method::POST, &route, None, &body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send_json(Method::POST, &route, Some(&fixture.user_token), &body)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send_json(
                Method::POST,
                &route,
                Some(&fixture.manager_token),
                &json!({
                    "Language": "eng",
                    "Format": "srt",
                    "IsForced": false,
                    "IsHearingImpaired": false,
                    "Data": "not-base64"
                }),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .send_json(
                Method::POST,
                &Fixture::upload_route(Uuid::new_v4()),
                Some(&fixture.manager_token),
                &body,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send_json(
                Method::POST,
                &Fixture::upload_route(fixture.folder_id),
                Some(&fixture.manager_token),
                &body,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send_json(Method::POST, &route, Some(&fixture.manager_token), &body)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("streams after upload");
    let uploaded = streams
        .iter()
        .find(|stream| stream.index == 4)
        .expect("uploaded subtitle stream");
    assert_eq!(uploaded.stream_type, MediaStreamType::Subtitle);
    assert_eq!(uploaded.codec.as_deref(), Some("srt"));
    assert_eq!(uploaded.language.as_deref(), Some("eng"));
    assert!(uploaded.is_external);
    assert!(uploaded.is_forced);
    assert!(!uploaded.is_hearing_impaired);
    let path = uploaded.path.as_deref().expect("uploaded subtitle path");
    assert!(path.contains("/subtitles/"));
    assert!(path.ends_with("/4.eng.srt"));
    let bytes = tokio::fs::read(path).await.expect("uploaded subtitle file");
    assert_eq!(
        Bytes::from(bytes),
        Bytes::from_static(b"1\n00:00:01,000 --> 00:00:02,000\nHello from upload\n")
    );

    let direct = fixture
        .send(
            Method::GET,
            &Fixture::stream_route(fixture.item_id, 4, "srt"),
            None,
        )
        .await;
    assert_eq!(direct.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(direct).await,
        Bytes::from_static(b"1\n00:00:01,000 --> 00:00:02,000\nHello from upload\n")
    );
    let alternate_source = fixture
        .alternate_id
        .simple()
        .to_string()
        .to_ascii_uppercase();
    let alternate = fixture
        .send(
            Method::GET,
            &Fixture::stream_route_with_source(fixture.item_id, &alternate_source, 4, "srt"),
            None,
        )
        .await;
    assert_eq!(alternate.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(alternate).await,
        Bytes::from_static(b"alternate subtitle bytes\n")
    );
    let lowercase_alternate = fixture
        .send(
            Method::GET,
            &format!(
                "/videos/{}/{alternate_source}/subtitles/4/stream.srt",
                fixture.item_id
            ),
            None,
        )
        .await;
    assert_eq!(lowercase_alternate.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(lowercase_alternate).await,
        Bytes::from_static(b"alternate subtitle bytes\n")
    );
    let alternate_with_ticks = fixture
        .send(
            Method::GET,
            &format!(
                "/Videos/{}/{alternate_source}/Subtitles/4/10000000/Stream.srt",
                fixture.item_id
            ),
            None,
        )
        .await;
    assert_eq!(alternate_with_ticks.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(alternate_with_ticks).await,
        Bytes::from_static(b"alternate subtitle bytes\n")
    );
    for query in [
        format!(
            "ItemId={}&MediaSourceId={alternate_source}&Index=4&Format=srt",
            fixture.item_id
        ),
        format!(
            "itemId={}&mediaSourceId={alternate_source}&index=4&format=srt",
            fixture.item_id
        ),
        format!(
            "itemid={}&mediasourceid={alternate_source}&index=4&format=srt",
            fixture.item_id
        ),
    ] {
        let response = fixture
            .send(
                Method::GET,
                &format!(
                    "/Videos/{}/{}/Subtitles/99/Stream.vtt?{query}",
                    fixture.outsider_id,
                    fixture.outsider_id.simple()
                ),
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{query}");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"alternate subtitle bytes\n"),
            "{query}"
        );
    }
    for source in [
        fixture.outsider_id.simple().to_string(),
        fixture.alternate_id.to_string(),
        "not-a-media-source".to_owned(),
    ] {
        assert_eq!(
            fixture
                .send(
                    Method::GET,
                    &Fixture::stream_route_with_source(fixture.item_id, &source, 4, "srt"),
                    None,
                )
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{source}"
        );
    }

    for query in [
        "addVttTimeMap=true&startPositionTicks=10000000",
        "AddVttTimeMap=true&StartPositionTicks=10000000",
        "addvtttimemap=true&startpositionticks=10000000",
    ] {
        let converted = fixture
            .send(
                Method::GET,
                &format!(
                    "{}?{query}",
                    Fixture::stream_route(fixture.item_id, 4, "vtt")
                ),
                None,
            )
            .await;
        assert_eq!(converted.status(), StatusCode::OK, "{query}");
        assert_eq!(
            body_bytes(converted).await,
            Bytes::from_static(
                b"WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:90000,LOCAL:00:00:00.000\n\n00:00:01.000 --> 00:00:02.000\nHello from upload\n"
            ),
            "{query}"
        );
    }

    let converted_from_ticks_route = fixture
        .send(
            Method::GET,
            &format!(
                "{}?addVttTimeMap=true",
                Fixture::stream_with_ticks_route(fixture.item_id, 4, 10_000_000, "vtt")
            ),
            None,
        )
        .await;
    assert_eq!(converted_from_ticks_route.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(converted_from_ticks_route).await,
        Bytes::from_static(
            b"WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:90000,LOCAL:00:00:00.000\n\n00:00:01.000 --> 00:00:02.000\nHello from upload\n"
        )
    );

    assert_eq!(
        fixture
            .send(
                Method::GET,
                &Fixture::stream_route(Uuid::new_v4(), 4, "srt"),
                None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let playlist_route = Fixture::playlist_route(fixture.item_id, 4, 10);
    assert_eq!(
        fixture
            .send(Method::GET, &playlist_route, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let playlist = fixture
        .send(Method::GET, &playlist_route, Some(&fixture.manager_token))
        .await;
    assert_eq!(playlist.status(), StatusCode::OK);
    let expected_playlist = format!(
        "#EXTM3U\n\
         #EXT-X-TARGETDURATION:10\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-MEDIA-SEQUENCE:0\n\
         #EXT-X-PLAYLIST-TYPE:VOD\n\
         #EXTINF:10,\n\
         stream.vtt?CopyTimestamps=true&AddVttTimeMap=true&StartPositionTicks=0&EndPositionTicks=100000000&ApiKey={}\n\
         #EXTINF:10,\n\
         stream.vtt?CopyTimestamps=true&AddVttTimeMap=true&StartPositionTicks=100000000&EndPositionTicks=200000000&ApiKey={}\n\
         #EXTINF:5,\n\
         stream.vtt?CopyTimestamps=true&AddVttTimeMap=true&StartPositionTicks=200000000&EndPositionTicks=250000000&ApiKey={}\n\
         #EXT-X-ENDLIST\n",
        fixture.manager_token, fixture.manager_token, fixture.manager_token
    );
    assert_eq!(
        String::from_utf8(body_bytes(playlist).await.to_vec()).expect("playlist text"),
        expected_playlist
    );
    let alternate_playlist = fixture
        .send(
            Method::GET,
            &Fixture::playlist_route_with_source(
                fixture.item_id,
                &fixture.alternate_id.simple().to_string(),
                4,
                10,
            ),
            Some(&fixture.manager_token),
        )
        .await;
    assert_eq!(alternate_playlist.status(), StatusCode::OK);
    let expected_alternate_playlist = format!(
        "#EXTM3U\n\
         #EXT-X-TARGETDURATION:10\n\
         #EXT-X-VERSION:3\n\
         #EXT-X-MEDIA-SEQUENCE:0\n\
         #EXT-X-PLAYLIST-TYPE:VOD\n\
         #EXTINF:10,\n\
         stream.vtt?CopyTimestamps=true&AddVttTimeMap=true&StartPositionTicks=0&EndPositionTicks=100000000&ApiKey={}\n\
         #EXTINF:2,\n\
         stream.vtt?CopyTimestamps=true&AddVttTimeMap=true&StartPositionTicks=100000000&EndPositionTicks=120000000&ApiKey={}\n\
         #EXT-X-ENDLIST\n",
        fixture.manager_token, fixture.manager_token
    );
    assert_eq!(
        String::from_utf8(body_bytes(alternate_playlist).await.to_vec())
            .expect("alternate playlist text"),
        expected_alternate_playlist
    );
    for route in [
        Fixture::playlist_route_with_source(
            fixture.item_id,
            &fixture.alternate_id.simple().to_string(),
            99,
            10,
        ),
        format!(
            "/videos/{}/{}/subtitles/99/subtitles.m3u8?segmentLength=10",
            fixture.item_id,
            fixture.alternate_id.simple()
        ),
    ] {
        let response = fixture
            .send(Method::GET, &route, Some(&fixture.manager_token))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(
            String::from_utf8(body_bytes(response).await.to_vec())
                .expect("alternate playlist text"),
            expected_alternate_playlist,
            "{route}"
        );
    }
    for source in [
        fixture.outsider_id.simple().to_string(),
        fixture.alternate_id.to_string(),
        "not-a-media-source".to_owned(),
    ] {
        assert_eq!(
            fixture
                .send(
                    Method::GET,
                    &Fixture::playlist_route_with_source(fixture.item_id, &source, 4, 10),
                    Some(&fixture.manager_token),
                )
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{source}"
        );
    }
    for query_name in ["SegmentLength", "segmentlength"] {
        let playlist = fixture
            .send(
                Method::GET,
                &format!(
                    "/Videos/{}/{}/Subtitles/4/subtitles.m3u8?{query_name}=10",
                    fixture.item_id,
                    fixture.item_id.simple()
                ),
                Some(&fixture.manager_token),
            )
            .await;
        assert_eq!(playlist.status(), StatusCode::OK, "{query_name}");
        assert_eq!(
            String::from_utf8(body_bytes(playlist).await.to_vec()).expect("playlist text"),
            expected_playlist,
            "{query_name}"
        );
    }
    assert_eq!(
        fixture
            .send(
                Method::GET,
                &Fixture::playlist_route(fixture.item_id, 4, 0),
                Some(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .send(
                Method::GET,
                &Fixture::playlist_route(Uuid::new_v4(), 4, 10),
                Some(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send(
                Method::GET,
                &Fixture::playlist_route(fixture.outsider_id, 4, 10),
                Some(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    manager_id: Uuid,
    manager_token: String,
    item_id: Uuid,
    alternate_id: Uuid,
    outsider_id: Uuid,
    folder_id: Uuid,
    storage_root: PathBuf,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 4,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");

        let suffix = Uuid::new_v4().simple().to_string();
        let storage_root = std::env::temp_dir().join(format!("jellyfin-subtitle-routes-{suffix}"));
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("subtitle-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("subtitle-user-{suffix}"))
            .await
            .expect("user creation");
        let manager = users
            .create(&format!("subtitle-manager-{suffix}"))
            .await
            .expect("manager creation");
        users
            .update_policy(manager.id, &subtitle_manager_policy())
            .await
            .expect("subtitle manager policy");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = devices
            .create_session(NewDevice::new(
                admin.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-admin-{suffix}"),
            ))
            .await
            .expect("admin session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;
        let manager_token = devices
            .create_session(NewDevice::new(
                manager.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-manager-{suffix}"),
            ))
            .await
            .expect("manager session")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Subtitle Movie {suffix}"));
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Subtitle Movie {suffix}.mkv"));
        item.runtime_ticks = Some(25 * 10_000_000);
        let items = BaseItemRepository::new(database.clone());
        let item = items.create(item).await.expect("movie item creation");
        let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
        alternate.name = Some(format!("Subtitle Movie Alternate {suffix}"));
        alternate.media_type = Some("Video".to_owned());
        alternate.path = Some(format!("/media/Subtitle Movie {suffix} - 4K.mkv"));
        alternate.runtime_ticks = Some(12 * 10_000_000);
        let alternate = items
            .create(alternate)
            .await
            .expect("alternate movie item creation");
        let mut outsider = NewBaseItem::new(Uuid::new_v4(), "Movie");
        outsider.name = Some(format!("Subtitle Outsider {suffix}"));
        outsider.media_type = Some("Video".to_owned());
        outsider.path = Some(format!("/media/Subtitle Outsider {suffix}.mkv"));
        outsider.runtime_ticks = Some(8 * 10_000_000);
        let outsider = items
            .create(outsider)
            .await
            .expect("outsider movie item creation");
        ItemValueRepository::new(database.clone())
            .link(
                outsider.id,
                item_value::ItemValueType::Tags,
                "BlockedSubtitle",
            )
            .await
            .expect("blocked outsider tag");
        items
            .assign_local_alternate_versions(&[(alternate.id, item.id)])
            .await
            .expect("alternate movie assignment");
        let mut folder = NewBaseItem::new(Uuid::new_v4(), "Folder");
        folder.name = Some(format!("Subtitle Folder {suffix}"));
        folder.is_folder = true;
        let folder = BaseItemRepository::new(database.clone())
            .create(folder)
            .await
            .expect("folder item creation");
        let media_streams = MediaStreamService::new(database.clone());
        media_streams
            .save_media_streams(
                item.id,
                vec![
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Video,
                        codec: Some("h264".to_owned()),
                        path: Some(format!("/media/Subtitle Movie {suffix}.mkv")),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("eng".to_owned()),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("srt".to_owned()),
                        language: Some("eng".to_owned()),
                        is_external: true,
                        path: Some(format!("/media/Subtitle Movie {suffix}.eng.srt")),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 3,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("ass".to_owned()),
                        language: Some("jpn".to_owned()),
                        is_external: true,
                        path: Some(format!("/media/Subtitle Movie {suffix}.jpn.ass")),
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("media stream creation");
        let alternate_subtitle_path = storage_root.join("alternate.srt");
        tokio::fs::create_dir_all(&storage_root)
            .await
            .expect("subtitle fixture directory");
        tokio::fs::write(&alternate_subtitle_path, b"alternate subtitle bytes\n")
            .await
            .expect("alternate subtitle fixture file");
        let outsider_subtitle_path = storage_root.join("outsider.srt");
        tokio::fs::write(&outsider_subtitle_path, b"outsider subtitle bytes\n")
            .await
            .expect("outsider subtitle fixture file");
        for (source_id, path) in [
            (alternate.id, alternate_subtitle_path),
            (outsider.id, outsider_subtitle_path),
        ] {
            media_streams
                .save_media_streams(
                    source_id,
                    vec![MediaStream {
                        index: 4,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("srt".to_owned()),
                        language: Some("eng".to_owned()),
                        is_external: true,
                        path: Some(path.to_string_lossy().into_owned()),
                        ..MediaStream::default()
                    }],
                )
                .await
                .expect("version subtitle stream creation");
        }

        let app_state = AppState::new(
            database.clone(),
            "Subtitle Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            storage_root.join("programdata"),
            storage_root.join("web"),
            storage_root.join("cache").join("images"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
        );
        let app = jellyfin_api::router(app_state);
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            manager_id: manager.id,
            manager_token,
            item_id: item.id,
            alternate_id: alternate.id,
            outsider_id: outsider.id,
            folder_id: folder.id,
            storage_root,
        }
    }

    fn subtitle_route(item_id: Uuid, index: i32) -> String {
        format!("/Videos/{item_id}/Subtitles/{index}")
    }

    fn search_route(item_id: Uuid, language: &str) -> String {
        format!("/Items/{item_id}/RemoteSearch/Subtitles/{language}?isPerfectMatch=true")
    }

    fn download_route(item_id: Uuid, subtitle_id: &str) -> String {
        format!("/Items/{item_id}/RemoteSearch/Subtitles/{subtitle_id}")
    }

    fn upload_route(item_id: Uuid) -> String {
        format!("/Videos/{item_id}/Subtitles")
    }

    fn stream_route(item_id: Uuid, index: i32, format: &str) -> String {
        format!(
            "/Videos/{item_id}/{}/Subtitles/{index}/Stream.{format}",
            item_id.simple()
        )
    }

    fn stream_route_with_source(
        item_id: Uuid,
        media_source_id: &str,
        index: i32,
        format: &str,
    ) -> String {
        format!("/Videos/{item_id}/{media_source_id}/Subtitles/{index}/Stream.{format}")
    }

    fn stream_with_ticks_route(
        item_id: Uuid,
        index: i32,
        start_position_ticks: i64,
        format: &str,
    ) -> String {
        format!(
            "/Videos/{item_id}/{}/Subtitles/{index}/{start_position_ticks}/Stream.{format}",
            item_id.simple()
        )
    }

    fn playlist_route(item_id: Uuid, index: i32, segment_length: i64) -> String {
        format!(
            "/Videos/{item_id}/{}/Subtitles/{index}/subtitles.m3u8?segmentLength={segment_length}",
            item_id.simple()
        )
    }

    fn playlist_route_with_source(
        item_id: Uuid,
        media_source_id: &str,
        index: i32,
        segment_length: i64,
    ) -> String {
        format!(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/subtitles.m3u8?segmentLength={segment_length}"
        )
    }

    async fn send(
        &self,
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
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn send_json(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: &Value,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(body).expect("request JSON")))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        BaseItemRepository::new(self.database.clone())
            .delete_many(&[
                self.item_id,
                self.alternate_id,
                self.outsider_id,
                self.folder_id,
            ])
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id, self.manager_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        let _ = tokio::fs::remove_dir_all(&self.storage_root).await;
        self.database.close().await.unwrap();
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
}

fn subtitle_manager_policy() -> UserPolicy {
    UserPolicy {
        enable_subtitle_management: true,
        blocked_tags: vec!["BlockedSubtitle".to_owned()],
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
