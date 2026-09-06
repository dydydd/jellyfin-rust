use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{
    LyricProvider, LyricProviderFuture, LyricSearchRequest, MediaAttachmentService,
    MediaStreamService, RemoteLyricInfo, RemoteLyricResponse, UserService,
};
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, ItemValueRepository, NewBaseItem, NewDevice,
    NewTrickplayInfo, NewUserData, TrickplayInfoRepository, USER_ROOT_FOLDER_ID,
    UserDataRepository,
    entities::{base_item, item_value, user},
};
use jellyfin_model::{MediaAttachment, MediaStream, MediaStreamType, UserPolicy};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User Library Tests\", Device=\"Test\", DeviceId=\"user-library\", Version=\"1.0\"";
const DOWNLOAD_LYRIC_PROVIDER_ID: &str = "a14e566951f4669abf58f6555ec9d3d1";

struct RemoteDownloadLyricProvider {
    content: Vec<u8>,
    requested_ids: Arc<Mutex<Vec<String>>>,
}

impl LyricProvider for RemoteDownloadLyricProvider {
    fn name(&self) -> &str {
        "Download Provider"
    }

    fn search<'a>(
        &'a self,
        _request: &'a LyricSearchRequest,
    ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn get_lyrics<'a>(
        &'a self,
        id: &'a str,
    ) -> LyricProviderFuture<'a, Option<RemoteLyricResponse>> {
        self.requested_ids
            .lock()
            .expect("requested lyric ids")
            .push(id.to_owned());
        let content = self.content.clone();
        Box::pin(async move {
            match id {
                "missing" => Ok(None),
                "invalid" => Ok(Some(RemoteLyricResponse::new(
                    "srt",
                    b"unsupported".to_vec(),
                ))),
                "error" => Err(std::io::Error::other("provider download failed").into()),
                _ => Ok(Some(RemoteLyricResponse::new("lrc", content))),
            }
        })
    }
}

#[tokio::test]
async fn official_nonexistent_user_routes_return_not_found() {
    let fixture = UserLibraryFixture::new().await;
    let missing_user_id = Uuid::new_v4();
    let routes = [
        format!("/Users/{missing_user_id}/Items/Root"),
        format!("/Users/{missing_user_id}/Items/{}", fixture.root_id),
        format!("/Users/{missing_user_id}/Items/{}/Intros", fixture.root_id),
        format!(
            "/Users/{missing_user_id}/Items/{}/LocalTrailers",
            fixture.root_id
        ),
        format!(
            "/Users/{missing_user_id}/Items/{}/SpecialFeatures",
            fixture.root_id
        ),
        format!("/Users/{missing_user_id}/Items/{}/Lyrics", fixture.root_id),
    ];
    for route in routes {
        let response = request(&fixture.app, &route, &fixture.administrator_token).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn official_nonexistent_item_routes_return_not_found() {
    let fixture = UserLibraryFixture::new().await;
    let item_id = Uuid::new_v4();
    for suffix in [
        "",
        "/Intros",
        "/LocalTrailers",
        "/SpecialFeatures",
        "/Lyrics",
    ] {
        let route = format!(
            "/Users/{}/Items/{item_id}{suffix}",
            fixture.administrator_id
        );
        let response = request(&fixture.app, &route, &fixture.administrator_token).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn valid_legacy_routes_cover_the_flaky_official_success_paths() {
    let fixture = UserLibraryFixture::new().await;

    let root_route = format!("/Users/{}/Items/Root", fixture.user_id);
    let root = get_json(&fixture.app, &root_route, &fixture.user_token).await;
    assert_base_item(&root, fixture.root_id, "UserRootFolder", "Root");

    let item_route = format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_id);
    let item = get_json(&fixture.app, &item_route, &fixture.user_token).await;
    assert_base_item(&item, fixture.item_id, "Audio", "Test Song");
    assert_eq!(item["HasLyrics"], false);
    assert!(item.get("item_type").is_none());

    let intros = get_json(
        &fixture.app,
        &format!("{item_route}/Intros"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(intros["TotalRecordCount"], 1);
    assert_eq!(intros["StartIndex"], 0);
    assert_eq!(
        intros["Items"][0]["Id"],
        fixture.intro_id.simple().to_string()
    );
    assert!(intros.get("total_record_count").is_none());

    let trailers = get_json(
        &fixture.app,
        &format!("{item_route}/LocalTrailers"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(trailers.as_array().unwrap().len(), 1);
    assert_eq!(trailers[0]["Id"], fixture.trailer_id.simple().to_string());
    assert_eq!(trailers[0]["ExtraType"], "Trailer");

    let features = get_json(
        &fixture.app,
        &format!("{item_route}/SpecialFeatures"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(features.as_array().unwrap().len(), 1);
    assert_eq!(features[0]["Id"], fixture.feature_id.simple().to_string());
    assert_eq!(features[0]["ExtraType"], "Featurette");

    let lyrics = get_json(
        &fixture.app,
        &format!("{item_route}/Lyrics"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(lyrics["Metadata"]["Artist"], "Test Artist");
    assert_eq!(lyrics["Lyrics"][0]["Text"], "First line");

    fixture.cleanup().await;
}

#[tokio::test]
async fn related_item_routes_batch_default_fields_and_enforce_target_user_visibility() {
    let fixture = UserLibraryFixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let marker = Uuid::new_v4().simple().to_string();

    let mut intro = items
        .get(fixture.intro_id)
        .await
        .expect("intro lookup")
        .expect("intro item");
    intro.media_type = Some("Video".to_owned());
    intro.path = Some(format!("/media/{marker}-intro.mkv"));
    intro.data = Some(json!({ "IsIntro": true, "HasSubtitles": false }));
    let intro = items.update(intro).await.expect("intro update");

    let mut trailer = items
        .get(fixture.trailer_id)
        .await
        .expect("trailer lookup")
        .expect("trailer item");
    trailer.media_type = Some("Video".to_owned());
    trailer.path = Some(format!("/media/{marker}-trailer.mkv"));
    trailer.data = Some(json!({ "ExtraType": "Trailer", "HasSubtitles": true }));
    let trailer = items.update(trailer).await.expect("trailer update");

    let mut audio_feature = item(
        "Audio",
        "Audio Feature With Lyrics",
        Some(fixture.item_id),
        false,
    );
    audio_feature.media_type = Some("Audio".to_owned());
    audio_feature.path = Some(format!("/media/{marker}-feature.flac"));
    audio_feature.data = Some(json!({ "ExtraType": "Featurette" }));
    let audio_feature = items
        .create(audio_feature)
        .await
        .expect("audio special feature");

    let streams = MediaStreamService::new(fixture.database.clone());
    streams
        .save_media_streams(
            intro.id,
            vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    path: intro.path.clone(),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Subtitle,
                    codec: Some("ass".to_owned()),
                    language: Some("jpn".to_owned()),
                    path: intro.path.clone(),
                    ..MediaStream::default()
                },
            ],
        )
        .await
        .expect("intro streams");
    streams
        .save_media_streams(
            trailer.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("hevc".to_owned()),
                path: trailer.path.clone(),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("trailer stream");
    streams
        .save_media_streams(
            audio_feature.id,
            vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("flac".to_owned()),
                    path: audio_feature.path.clone(),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Lyric,
                    codec: Some("lrc".to_owned()),
                    path: audio_feature.path.clone(),
                    ..MediaStream::default()
                },
            ],
        )
        .await
        .expect("feature lyric stream");

    for route in [
        format!("/Items/{}/Intros", fixture.item_id),
        format!(
            "/Users/{}/Items/{}/Intros",
            fixture.user_id, fixture.item_id
        ),
    ] {
        let response = get_json(&fixture.app, &route, &fixture.user_token).await;
        let dto = &response["Items"][0];
        assert_eq!(response["TotalRecordCount"], 1, "{route}");
        assert_eq!(dto["HasSubtitles"], true, "{route}");
        assert_eq!(dto["MediaStreams"].as_array().unwrap().len(), 2, "{route}");
        assert_eq!(dto["MediaStreams"][1]["Language"], "jpn", "{route}");
        assert_eq!(dto["MediaSources"].as_array().unwrap().len(), 1, "{route}");
        assert!(dto["UserData"].is_object(), "{route}");
    }

    for route in [
        format!("/Items/{}/LocalTrailers", fixture.item_id),
        format!(
            "/Users/{}/Items/{}/LocalTrailers",
            fixture.user_id, fixture.item_id
        ),
    ] {
        let response = get_json(&fixture.app, &route, &fixture.user_token).await;
        let dto = &response[0];
        assert_eq!(response.as_array().unwrap().len(), 1, "{route}");
        assert!(dto.get("HasSubtitles").is_none(), "{route}: {dto}");
        assert_eq!(dto["MediaStreams"].as_array().unwrap().len(), 1, "{route}");
        assert_eq!(dto["MediaSources"].as_array().unwrap().len(), 1, "{route}");
        assert!(dto["UserData"].is_object(), "{route}");
    }

    for route in [
        format!("/Items/{}/SpecialFeatures", fixture.item_id),
        format!(
            "/Users/{}/Items/{}/SpecialFeatures",
            fixture.user_id, fixture.item_id
        ),
    ] {
        let response = get_json(&fixture.app, &route, &fixture.user_token).await;
        let dto = response
            .as_array()
            .unwrap()
            .iter()
            .find(|dto| dto["Id"] == audio_feature.id.simple().to_string())
            .expect("audio feature DTO");
        assert_eq!(dto["HasLyrics"], true, "{route}");
        assert_eq!(dto["MediaStreams"].as_array().unwrap().len(), 2, "{route}");
        assert_eq!(dto["MediaStreams"][0]["Type"], "Audio", "{route}");
        assert_eq!(dto["MediaStreams"][1]["Type"], "Lyric", "{route}");
        assert_eq!(dto["MediaSources"].as_array().unwrap().len(), 1, "{route}");
        assert_eq!(
            dto["MediaSources"][0]["MediaStreams"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "{route}"
        );
        assert_eq!(
            dto["MediaSources"][0]["MediaStreams"][0]["Type"], "Audio",
            "{route}"
        );
        assert_eq!(
            dto["MediaSources"][0]["MediaStreams"][1]["Type"], "Lyric",
            "{route}"
        );
    }

    let values = ItemValueRepository::new(fixture.database.clone());
    values
        .link(
            trailer.id,
            item_value::ItemValueType::Tags,
            "BlockedRelated",
        )
        .await
        .expect("blocked trailer tag");
    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.blocked_tags = vec!["BlockedRelated".to_owned()];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("related-item user policy");

    let hidden_children = get_json(
        &fixture.app,
        &format!("/Items/{}/LocalTrailers", fixture.item_id),
        &fixture.user_token,
    )
    .await;
    assert!(hidden_children.as_array().unwrap().is_empty());
    let administrator_children = get_json(
        &fixture.app,
        &format!("/Items/{}/LocalTrailers", fixture.item_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(administrator_children.as_array().unwrap().len(), 1);

    values
        .link(
            fixture.item_id,
            item_value::ItemValueType::Tags,
            "BlockedRelated",
        )
        .await
        .expect("blocked related owner tag");
    assert_eq!(
        request(
            &fixture.app,
            &format!("/Items/{}/Intros", fixture.item_id),
            &fixture.user_token,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn episode_detail_routes_project_official_series_and_season_names() {
    let fixture = UserLibraryFixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let series = items
        .create(item(
            "Series",
            "Example Series",
            Some(fixture.root_id),
            true,
        ))
        .await
        .expect("series item");
    let mut season = item("Season", "Season 2", Some(series.id), true);
    season.series_id = Some(series.id);
    let season = items.create(season).await.expect("season item");
    let mut episode = item("Episode", "Episode 3", Some(season.id), false);
    episode.media_type = Some("Video".to_owned());
    episode.series_id = Some(series.id);
    episode.season_id = Some(season.id);
    let episode = items.create(episode).await.expect("episode item");

    for route in [
        format!("/Items/{}", episode.id),
        format!("/Users/{}/Items/{}", fixture.user_id, episode.id),
    ] {
        let dto = get_json(&fixture.app, &route, &fixture.user_token).await;
        assert_eq!(dto["SeriesName"], "Example Series", "{route}");
        assert_eq!(dto["SeasonName"], "Season 2", "{route}");
        assert_eq!(dto["SeriesId"], series.id.simple().to_string(), "{route}");
        assert_eq!(dto["SeasonId"], season.id.simple().to_string(), "{route}");
    }
    let page = get_json(
        &fixture.app,
        &format!(
            "/Items?userId={}&parentId={}&includeItemTypes=Episode",
            fixture.user_id, season.id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(page["Items"].as_array().unwrap().len(), 1);
    assert_eq!(page["Items"][0]["SeriesName"], "Example Series");
    assert_eq!(page["Items"][0]["SeasonName"], "Season 2");

    let episodes = get_json(
        &fixture.app,
        &format!("/Shows/{}/Episodes?userId={}", series.id, fixture.user_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(episodes["Items"].as_array().unwrap().len(), 1);
    assert_eq!(episodes["Items"][0]["SeriesName"], "Example Series");
    assert_eq!(episodes["Items"][0]["SeasonName"], "Season 2");

    items
        .delete_many(&[episode.id, season.id, series.id])
        .await
        .expect("episode hierarchy cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn audio_details_and_lists_project_album_and_artist_fields() {
    let fixture = UserLibraryFixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let values = ItemValueRepository::new(fixture.database.clone());
    let artist = items
        .create(item(
            "MusicArtist",
            "Library Artist",
            Some(fixture.root_id),
            true,
        ))
        .await
        .expect("music artist");
    let mut album = item("MusicAlbum", "Nested Album", Some(artist.id), true);
    album.data = Some(json!({
        "Artists": ["Zulu Guest", "Alpha Guest", "Zulu Guest"],
        "AlbumArtists": ["Primary Artist", "Second Artist", "Primary Artist"]
    }));
    let album = items.create(album).await.expect("music album");
    let intermediate = items
        .create(item("Folder", "Disc 1", Some(album.id), true))
        .await
        .expect("intermediate album folder");
    let mut audio = item("Audio", "Nested Song", Some(intermediate.id), false);
    audio.media_type = Some("Audio".to_owned());
    audio.data = Some(json!({
        "Album": "Nested Album",
        "Artists": ["Zulu Guest", "Alpha Guest", "Zulu Guest"],
        "AlbumArtists": ["Primary Artist", "Second Artist", "Primary Artist"]
    }));
    let audio = items.create(audio).await.expect("audio item");

    let zulu = values
        .link(audio.id, item_value::ItemValueType::Artist, "Zulu Guest")
        .await
        .expect("zulu artist relation");
    let alpha = values
        .link(audio.id, item_value::ItemValueType::Artist, "Alpha Guest")
        .await
        .expect("alpha artist relation");
    let primary = values
        .link(
            audio.id,
            item_value::ItemValueType::AlbumArtist,
            "Primary Artist",
        )
        .await
        .expect("primary album artist relation");
    let second = values
        .link(
            audio.id,
            item_value::ItemValueType::AlbumArtist,
            "Second Artist",
        )
        .await
        .expect("second album artist relation");

    for route in [
        format!("/Items/{}", audio.id),
        format!("/Users/{}/Items/{}", fixture.user_id, audio.id),
    ] {
        let dto = get_json(&fixture.app, &route, &fixture.user_token).await;
        assert_eq!(dto["Album"], "Nested Album", "{route}");
        assert_eq!(dto["AlbumId"], album.id.simple().to_string(), "{route}");
        assert_eq!(
            dto["Artists"],
            json!(["Zulu Guest", "Alpha Guest", "Zulu Guest"]),
            "{route}"
        );
        assert_eq!(
            dto["ArtistItems"],
            json!([
                { "Name": "Zulu Guest", "Id": zulu.item_value_id.simple().to_string() },
                { "Name": "Alpha Guest", "Id": alpha.item_value_id.simple().to_string() }
            ]),
            "{route}"
        );
        assert_eq!(dto["AlbumArtist"], "Primary Artist", "{route}");
        assert_eq!(
            dto["AlbumArtists"],
            json!([
                { "Name": "Primary Artist", "Id": primary.item_value_id.simple().to_string() },
                { "Name": "Second Artist", "Id": second.item_value_id.simple().to_string() }
            ]),
            "{route}"
        );
    }

    let page = get_json(
        &fixture.app,
        &format!(
            "/Items?userId={}&parentId={}&includeItemTypes=Audio",
            fixture.user_id, intermediate.id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(page["Items"].as_array().unwrap().len(), 1);
    let dto = &page["Items"][0];
    assert_eq!(dto["Album"], "Nested Album");
    assert_eq!(dto["AlbumId"], album.id.simple().to_string());
    assert_eq!(
        dto["Artists"],
        json!(["Zulu Guest", "Alpha Guest", "Zulu Guest"])
    );
    assert_eq!(dto["AlbumArtist"], "Primary Artist");
    assert_eq!(dto["ArtistItems"].as_array().unwrap().len(), 2);
    assert_eq!(dto["AlbumArtists"].as_array().unwrap().len(), 2);

    items
        .delete(artist.id)
        .await
        .expect("music hierarchy cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn media_stream_fields_are_projected_for_single_item_routes() {
    let fixture = UserLibraryFixture::new().await;

    let routes = [
        format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_id),
        format!("/Items/{}?UserId={}", fixture.item_id, fixture.user_id),
    ];
    for route in routes {
        let item = get_json(&fixture.app, &route, &fixture.user_token).await;

        assert_eq!(item["MediaSources"].as_array().unwrap().len(), 1, "{route}");
        assert_eq!(item["MediaSources"][0]["Path"], "/media/Test Song.mkv");
        assert_eq!(item["MediaSources"][0]["Name"], "Test Song");
        assert_eq!(
            item["MediaSources"][0]["MediaStreams"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            item["MediaSources"][0]["MediaAttachments"][0]["FileName"],
            "poster.jpg"
        );
        assert_eq!(item["MediaSources"][0]["MediaAttachments"][0]["Index"], 4);
        assert_eq!(item["MediaStreams"].as_array().unwrap().len(), 1);
        assert_eq!(item["MediaSources"][0]["MediaStreams"][0]["Type"], "Audio");
        assert_eq!(item["MediaStreams"][0]["Type"], "Audio");
        assert_eq!(item["MediaStreams"][0]["Language"], "deu");
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn trickplay_is_projected_by_default_for_single_video_items() {
    let fixture = UserLibraryFixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let mut video = item("Movie", "Trickplay Video", Some(fixture.root_id), false);
    video.media_type = Some("Video".to_owned());
    let video = items.create(video).await.expect("video item");
    TrickplayInfoRepository::new(fixture.database.clone())
        .upsert(
            video.id,
            NewTrickplayInfo {
                width: 640,
                height: 360,
                tile_width: 5,
                tile_height: 5,
                thumbnail_count: 50,
                interval: 1_000,
                bandwidth: 88_000,
            },
        )
        .await
        .expect("trickplay metadata");

    let route = format!("/Users/{}/Items/{}", fixture.user_id, video.id);
    let projected = get_json(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(
        projected["Trickplay"][video.id.simple().to_string()]["640"]["Bandwidth"],
        88_000
    );

    let audio = get_json(
        &fixture.app,
        &format!(
            "/Users/{}/Items/{}?Fields=Trickplay",
            fixture.user_id, fixture.item_id
        ),
        &fixture.user_token,
    )
    .await;
    assert!(audio.get("Trickplay").is_none());

    items.delete(video.id).await.expect("video cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn remote_lyric_search_matches_management_policy_and_empty_provider_contract() {
    let fixture = UserLibraryFixture::new().await;
    let route = format!("/Audio/{}/RemoteSearch/Lyrics", fixture.item_id);

    let unauthenticated = fixture
        .app
        .clone()
        .oneshot(Request::get(&route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let regular_user = request(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(regular_user.status(), StatusCode::FORBIDDEN);

    let remote = get_json(&fixture.app, &route, &fixture.administrator_token).await;
    assert_eq!(remote.as_array().expect("remote lyric results").len(), 0);

    let missing = request(
        &fixture.app,
        &format!("/Audio/{}/RemoteSearch/Lyrics", Uuid::new_v4()),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let non_audio = request(
        &fixture.app,
        &format!("/Audio/{}/RemoteSearch/Lyrics", fixture.root_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(non_audio.status(), StatusCode::NOT_FOUND);

    let download_route = format!(
        "/Audio/{}/RemoteSearch/Lyrics/remote-provider-id",
        fixture.item_id
    );
    let download_unauthenticated = fixture
        .app
        .clone()
        .oneshot(Request::post(&download_route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(download_unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let download_regular = request_post(&fixture.app, &download_route, &fixture.user_token).await;
    assert_eq!(download_regular.status(), StatusCode::FORBIDDEN);
    let download_admin =
        request_post(&fixture.app, &download_route, &fixture.administrator_token).await;
    assert_eq!(download_admin.status(), StatusCode::NOT_FOUND);

    let download_missing = request_post(
        &fixture.app,
        &format!(
            "/Audio/{}/RemoteSearch/Lyrics/remote-provider-id",
            Uuid::new_v4()
        ),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(download_missing.status(), StatusCode::NOT_FOUND);

    let provider_route = "/Providers/Lyrics/remote-provider-id";
    let provider_regular = request(&fixture.app, provider_route, &fixture.user_token).await;
    assert_eq!(provider_regular.status(), StatusCode::FORBIDDEN);
    let provider_admin = request(&fixture.app, provider_route, &fixture.administrator_token).await;
    assert_eq!(provider_admin.status(), StatusCode::NOT_FOUND);

    fixture.cleanup().await;
}

#[tokio::test]
async fn remote_lyric_get_and_download_preserve_provider_bytes_and_error_semantics() {
    let original = utf16_lyric_bytes("[00:01.00]Downloaded from UTF-16", true);
    let requested_ids = Arc::new(Mutex::new(Vec::new()));
    let fixture =
        UserLibraryFixture::with_lyric_providers(vec![Arc::new(RemoteDownloadLyricProvider {
            content: original.clone(),
            requested_ids: Arc::clone(&requested_ids),
        })])
        .await;

    let preview_route =
        format!("/Providers/Lyrics/{DOWNLOAD_LYRIC_PROVIDER_ID}_preview_with_underscores");
    let preview = get_json(&fixture.app, &preview_route, &fixture.administrator_token).await;
    assert_eq!(preview["Lyrics"][0]["Text"], "Downloaded from UTF-16");
    assert!(
        MediaStreamService::new(fixture.database.clone())
            .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
                fixture.item_id,
            ))
            .await
            .expect("streams after preview")
            .iter()
            .all(|stream| stream.stream_type != MediaStreamType::Lyric),
        "provider preview must not persist lyrics"
    );

    let download_route = format!(
        "/Audio/{}/RemoteSearch/Lyrics/{DOWNLOAD_LYRIC_PROVIDER_ID}_download_with_underscores",
        fixture.item_id
    );
    let download = request_post(&fixture.app, &download_route, &fixture.administrator_token).await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        body_json(download).await["Lyrics"][0]["Text"],
        "Downloaded from UTF-16"
    );

    let lyric_stream = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("streams after download")
        .into_iter()
        .find(|stream| stream.stream_type == MediaStreamType::Lyric)
        .expect("registered lyric stream");
    assert_eq!(lyric_stream.codec.as_deref(), Some("lrc"));
    let lyric_path = lyric_stream.path.expect("managed lyric path");
    assert_eq!(
        tokio::fs::read(&lyric_path)
            .await
            .expect("persisted remote lyric"),
        original,
        "remote provider bytes must not be transcoded while saving"
    );

    for provider_result in ["missing", "invalid"] {
        let preview_response = request(
            &fixture.app,
            &format!("/Providers/Lyrics/{DOWNLOAD_LYRIC_PROVIDER_ID}_{provider_result}"),
            &fixture.administrator_token,
        )
        .await;
        assert_eq!(
            preview_response.status(),
            StatusCode::NOT_FOUND,
            "preview {provider_result}"
        );
        let download_response = request_post(
            &fixture.app,
            &format!(
                "/Audio/{}/RemoteSearch/Lyrics/{DOWNLOAD_LYRIC_PROVIDER_ID}_{provider_result}",
                fixture.item_id
            ),
            &fixture.administrator_token,
        )
        .await;
        assert_eq!(
            download_response.status(),
            StatusCode::NOT_FOUND,
            "download {provider_result}"
        );
    }
    let preview_error = request(
        &fixture.app,
        &format!("/Providers/Lyrics/{DOWNLOAD_LYRIC_PROVIDER_ID}_error"),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(preview_error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let download_error = request_post(
        &fixture.app,
        &format!(
            "/Audio/{}/RemoteSearch/Lyrics/{DOWNLOAD_LYRIC_PROVIDER_ID}_error",
            fixture.item_id
        ),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(download_error.status(), StatusCode::INTERNAL_SERVER_ERROR);

    assert_eq!(
        requested_ids
            .lock()
            .expect("requested lyric ids")
            .as_slice(),
        [
            "preview_with_underscores",
            "download_with_underscores",
            "missing",
            "missing",
            "invalid",
            "invalid",
            "error",
            "error"
        ]
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn upload_lyrics_matches_management_policy_and_persists_postgres_metadata() {
    let fixture = UserLibraryFixture::new().await;
    let route = format!("/Audio/{}/Lyrics?fileName=uploaded.txt", fixture.item_id);

    let unauthenticated = fixture
        .app
        .clone()
        .oneshot(
            Request::post(&route)
                .body(Body::from("Uploaded line"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let regular_user =
        request_post_body(&fixture.app, &route, &fixture.user_token, "Uploaded line").await;
    assert_eq!(regular_user.status(), StatusCode::FORBIDDEN);

    let empty = request_post_body(&fixture.app, &route, &fixture.administrator_token, "").await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let missing_extension = request_post_body(
        &fixture.app,
        &format!("/Audio/{}/Lyrics?fileName=uploaded", fixture.item_id),
        &fixture.administrator_token,
        "Uploaded line",
    )
    .await;
    assert_eq!(missing_extension.status(), StatusCode::BAD_REQUEST);

    let unsupported = request_post_body(
        &fixture.app,
        &format!("/Audio/{}/Lyrics?fileName=uploaded.srt", fixture.item_id),
        &fixture.administrator_token,
        "Uploaded line",
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);

    let traversal = request_post_body(
        &fixture.app,
        &format!("/Audio/{}/Lyrics?fileName=..%2Fevil.txt", fixture.item_id),
        &fixture.administrator_token,
        "Escaping line",
    )
    .await;
    assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);

    let missing = request_post_body(
        &fixture.app,
        &format!("/Audio/{}/Lyrics?fileName=uploaded.txt", Uuid::new_v4()),
        &fixture.administrator_token,
        "Uploaded line",
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let non_audio = request_post_body(
        &fixture.app,
        &format!("/Audio/{}/Lyrics?fileName=uploaded.txt", fixture.root_id),
        &fixture.administrator_token,
        "Uploaded line",
    )
    .await;
    assert_eq!(non_audio.status(), StatusCode::NOT_FOUND);

    for (target, label) in [
        (Uuid::new_v4(), "missing item"),
        (fixture.root_id, "non-audio item"),
    ] {
        let empty_invalid = request_post_body(
            &fixture.app,
            &format!("/Audio/{target}/Lyrics?fileName=uploaded.srt"),
            &fixture.administrator_token,
            "",
        )
        .await;
        assert_eq!(
            empty_invalid.status(),
            StatusCode::NOT_FOUND,
            "{label} must be resolved before validating upload contents"
        );
    }

    let uploaded = request_post_body(
        &fixture.app,
        &route,
        &fixture.administrator_token,
        "  First uploaded  \nSecond uploaded",
    )
    .await;
    assert_eq!(uploaded.status(), StatusCode::OK);
    let uploaded = body_json(uploaded).await;
    assert_eq!(uploaded["Metadata"], json!({}));
    assert_eq!(uploaded["Lyrics"][0]["Text"], "First uploaded");
    assert_eq!(uploaded["Lyrics"][1]["Text"], "Second uploaded");
    assert_eq!(uploaded["Lyrics"][0]["Start"], Value::Null);
    assert_eq!(uploaded["Lyrics"][0]["Cues"], Value::Null);

    let item_id = fixture.item_id.simple().to_string();
    let metadata_directory = fixture
        .storage_root
        .join("metadata/library")
        .join(&item_id[..2])
        .join(&item_id);
    let text_path = metadata_directory.join("Test Song.txt");
    assert_eq!(
        tokio::fs::read(&text_path).await.expect("uploaded lyric"),
        b"  First uploaded  \nSecond uploaded"
    );
    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("uploaded lyric streams");
    assert_eq!(
        streams
            .iter()
            .filter(|stream| stream.stream_type == MediaStreamType::Audio)
            .count(),
        1
    );
    let text_stream = streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Lyric)
        .expect("text lyric stream");
    assert_eq!(text_stream.codec.as_deref(), Some("txt"));
    assert_eq!(text_stream.path.as_deref(), text_path.to_str());

    let items = BaseItemRepository::new(fixture.database.clone());
    let mut stale = items
        .get(fixture.item_id)
        .await
        .expect("load item")
        .expect("stored item");
    stale.data.as_mut().unwrap()["Lyrics"] = json!({
        "Metadata": {},
        "Lyrics": [{ "Text": "Stale cache", "Start": null, "Cues": null }]
    });
    items.update(stale).await.expect("stale lyric cache");

    let saved = get_json(
        &fixture.app,
        &format!("/Audio/{}/Lyrics", fixture.item_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(saved["Lyrics"][0]["Text"], "First uploaded");

    let uploaded_lrc = request_post_body(
        &fixture.app,
        &format!("/Audio/{}/Lyrics?fileName=uploaded.lrc", fixture.item_id),
        &fixture.administrator_token,
        "[00:01.50]Synced later\n[00:00.25]Synced earlier",
    )
    .await;
    assert_eq!(uploaded_lrc.status(), StatusCode::OK);
    let uploaded_lrc = body_json(uploaded_lrc).await;
    assert_eq!(uploaded_lrc["Lyrics"][0]["Text"], "Synced earlier");
    assert_eq!(uploaded_lrc["Lyrics"][0]["Start"], 2_500_000);
    assert_eq!(uploaded_lrc["Lyrics"][0]["Cues"], json!([]));
    assert_eq!(uploaded_lrc["Lyrics"][1]["Text"], "Synced later");
    assert_eq!(uploaded_lrc["Lyrics"][1]["Start"], 15_000_000);
    assert_eq!(
        tokio::fs::read(metadata_directory.join("Test Song.lrc"))
            .await
            .expect("uploaded lrc"),
        b"[00:01.50]Synced later\n[00:00.25]Synced earlier"
    );
    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("multiple lyric streams");
    assert_eq!(
        streams
            .iter()
            .filter(|stream| stream.stream_type == MediaStreamType::Lyric)
            .count(),
        2
    );

    let item = get_json(
        &fixture.app,
        &format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(item["HasLyrics"], true);

    fixture.cleanup().await;
}

#[tokio::test]
async fn lyric_routes_accept_lowercase_paths_and_query_names() {
    let fixture = UserLibraryFixture::new().await;
    let lowercase_lyrics = format!("/audio/{}/lyrics", fixture.item_id);

    let original = get_json(&fixture.app, &lowercase_lyrics, &fixture.user_token).await;
    assert_eq!(original["Lyrics"][0]["Text"], "First line");

    let pascal_query = request_post_body(
        &fixture.app,
        &format!(
            "/Audio/{}/Lyrics?FileName=pascal-query.txt",
            fixture.item_id
        ),
        &fixture.administrator_token,
        "Pascal query",
    )
    .await;
    assert_eq!(pascal_query.status(), StatusCode::OK);

    let lowercase_query = request_post_body(
        &fixture.app,
        &format!("{lowercase_lyrics}?filename=lowercase-query.txt"),
        &fixture.administrator_token,
        "Lowercase query",
    )
    .await;
    assert_eq!(lowercase_query.status(), StatusCode::OK);
    assert_eq!(
        body_json(lowercase_query).await["Lyrics"][0]["Text"],
        "Lowercase query"
    );

    let search = request(
        &fixture.app,
        &format!("/audio/{}/remotesearch/lyrics", fixture.item_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(search.status(), StatusCode::OK);

    let download = request_post(
        &fixture.app,
        &format!("/audio/{}/remotesearch/lyrics/unavailable", fixture.item_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(download.status(), StatusCode::NOT_FOUND);

    let provider = request(
        &fixture.app,
        "/providers/lyrics/unavailable",
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(provider.status(), StatusCode::NOT_FOUND);

    let deleted = request_delete(
        &fixture.app,
        &lowercase_lyrics,
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    fixture.cleanup().await;
}

#[tokio::test]
async fn uploaded_lyrics_detect_boms_preserve_bytes_and_redecode_local_files() {
    let fixture = UserLibraryFixture::new().await;
    let item_id = fixture.item_id.simple().to_string();
    let lyric_path = fixture
        .storage_root
        .join("metadata/library")
        .join(&item_id[..2])
        .join(&item_id)
        .join("Test Song.txt");
    let utf8_bom = [b"\xEF\xBB\xBF".as_slice(), "UTF-8 歌词".as_bytes()].concat();
    let cases = [
        (
            format!("/audio/{}/lyrics?FileName=utf8-bom.txt", fixture.item_id),
            utf8_bom,
            "UTF-8 歌词",
        ),
        (
            format!("/Audio/{}/Lyrics?filename=utf16-le.txt", fixture.item_id),
            utf16_lyric_bytes("UTF-16LE 歌词", true),
            "UTF-16LE 歌词",
        ),
        (
            format!("/Audio/{}/Lyrics?fileName=utf16-be.txt", fixture.item_id),
            utf16_lyric_bytes("UTF-16BE 歌词", false),
            "UTF-16BE 歌词",
        ),
        (
            format!("/audio/{}/lyrics?FileName=utf32-le.txt", fixture.item_id),
            utf32_lyric_bytes("UTF-32LE 歌词", true),
            "UTF-32LE 歌词",
        ),
        (
            format!("/Audio/{}/Lyrics?filename=utf32-be.txt", fixture.item_id),
            utf32_lyric_bytes("UTF-32BE 歌词", false),
            "UTF-32BE 歌词",
        ),
        (
            format!("/audio/{}/lyrics?filename=no-bom.txt", fixture.item_id),
            "无 BOM 歌词".as_bytes().to_vec(),
            "无 BOM 歌词",
        ),
    ];

    for (route, payload, expected) in cases {
        let uploaded = request_post_bytes(
            &fixture.app,
            &route,
            &fixture.administrator_token,
            payload.clone(),
        )
        .await;
        assert_eq!(uploaded.status(), StatusCode::OK, "{route}");
        assert_eq!(
            body_json(uploaded).await["Lyrics"][0]["Text"],
            expected,
            "{route}"
        );
        assert_eq!(
            tokio::fs::read(&lyric_path).await.expect("uploaded lyric"),
            payload,
            "{route}"
        );

        let local = get_json(
            &fixture.app,
            &format!("/audio/{}/lyrics", fixture.item_id),
            &fixture.user_token,
        )
        .await;
        assert_eq!(local["Lyrics"][0]["Text"], expected, "{route}");
    }

    fixture.cleanup().await;
}

#[tokio::test]
async fn local_lyrics_try_streams_by_index_and_use_path_extensions() {
    let fixture = UserLibraryFixture::new().await;
    let item_id = fixture.item_id.simple().to_string();
    let metadata_directory = fixture
        .storage_root
        .join("metadata/library")
        .join(&item_id[..2])
        .join(&item_id);
    tokio::fs::create_dir_all(&metadata_directory)
        .await
        .expect("lyric metadata directory");
    let unsupported_path = metadata_directory.join("Test Song.srt");
    let valid_path = metadata_directory.join("Test Song.txt");
    tokio::fs::write(&unsupported_path, "Unsupported first stream")
        .await
        .expect("unsupported lyric stream");
    tokio::fs::write(&valid_path, "Valid later stream")
        .await
        .expect("valid lyric stream");

    let media_streams = MediaStreamService::new(fixture.database.clone());
    let mut streams = media_streams
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("existing media streams");
    streams.extend([
        MediaStream {
            codec: Some("txt".to_owned()),
            index: 1,
            stream_type: MediaStreamType::Lyric,
            is_external: true,
            path: Some(unsupported_path.to_string_lossy().into_owned()),
            ..MediaStream::default()
        },
        MediaStream {
            codec: Some("srt".to_owned()),
            index: 2,
            stream_type: MediaStreamType::Lyric,
            is_external: true,
            path: Some(valid_path.to_string_lossy().into_owned()),
            ..MediaStream::default()
        },
    ]);
    media_streams
        .save_media_streams(fixture.item_id, streams)
        .await
        .expect("multiple lyric streams");

    let route = format!("/Audio/{}/Lyrics", fixture.item_id);
    let lyrics = get_json(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(lyrics["Lyrics"][0]["Text"], "Valid later stream");

    let mut streams = media_streams
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("stored media streams");
    let missing_path = metadata_directory
        .join("missing.srt")
        .to_string_lossy()
        .into_owned();
    streams
        .iter_mut()
        .find(|stream| stream.index == 1)
        .expect("first lyric stream")
        .path = Some(missing_path);
    media_streams
        .save_media_streams(fixture.item_id, streams)
        .await
        .expect("missing first lyric stream");

    let missing_first = request(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(
        missing_first.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "filesystem errors from earlier streams must not be skipped"
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn delete_lyrics_matches_management_policy_and_updates_postgres_metadata() {
    let fixture = UserLibraryFixture::new().await;
    let route = format!("/Audio/{}/Lyrics", fixture.item_id);

    let before = get_json(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(before["Lyrics"][0]["Text"], "First line");

    let unauthenticated = fixture
        .app
        .clone()
        .oneshot(Request::delete(&route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let regular_user = request_delete(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(regular_user.status(), StatusCode::FORBIDDEN);

    let missing = request_delete(
        &fixture.app,
        &format!("/Audio/{}/Lyrics", Uuid::new_v4()),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let non_audio = request_delete(
        &fixture.app,
        &format!("/Audio/{}/Lyrics", fixture.root_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(non_audio.status(), StatusCode::NOT_FOUND);

    let deleted = request_delete(&fixture.app, &route, &fixture.administrator_token).await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let deleted_again = request_delete(&fixture.app, &route, &fixture.administrator_token).await;
    assert_eq!(deleted_again.status(), StatusCode::NO_CONTENT);

    let after = request(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);

    let item = get_json(
        &fixture.app,
        &format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(item["HasLyrics"], false);

    fixture.cleanup().await;
}

#[tokio::test]
async fn delete_lyrics_removes_only_registered_internal_files_and_preserves_other_streams() {
    let fixture = UserLibraryFixture::new().await;
    let route = format!("/Audio/{}/Lyrics?fileName=managed.txt", fixture.item_id);
    let uploaded = request_post_body(
        &fixture.app,
        &route,
        &fixture.administrator_token,
        "Managed lyric",
    )
    .await;
    assert_eq!(uploaded.status(), StatusCode::OK);

    let item_id = fixture.item_id.simple().to_string();
    let metadata_directory = fixture
        .storage_root
        .join("metadata/library")
        .join(&item_id[..2])
        .join(&item_id);
    let managed = metadata_directory.join("Test Song.txt");
    let unregistered = metadata_directory.join("unregistered.txt");
    tokio::fs::write(&unregistered, "keep sibling")
        .await
        .expect("unregistered lyric");
    let external = fixture.storage_root.join("outside-metadata.txt");
    tokio::fs::write(&external, "keep external")
        .await
        .expect("external lyric");

    let service = MediaStreamService::new(fixture.database.clone());
    let mut streams = service
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("stored streams");
    streams.push(MediaStream {
        codec: Some("txt".to_owned()),
        index: 9,
        stream_type: MediaStreamType::Lyric,
        is_external: true,
        path: Some(external.to_string_lossy().into_owned()),
        ..MediaStream::default()
    });
    service
        .save_media_streams(fixture.item_id, streams)
        .await
        .expect("external lyric stream");

    let deleted = request_delete(
        &fixture.app,
        &format!("/Audio/{}/Lyrics", fixture.item_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(!tokio::fs::try_exists(managed).await.unwrap());
    assert!(tokio::fs::try_exists(unregistered).await.unwrap());
    assert!(tokio::fs::try_exists(external).await.unwrap());

    let streams = service
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(
            fixture.item_id,
        ))
        .await
        .expect("remaining streams");
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].stream_type, MediaStreamType::Audio);

    fixture.cleanup().await;
}

#[tokio::test]
async fn lyric_routes_hide_policy_blocked_and_non_audio_items() {
    let fixture = UserLibraryFixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let values = ItemValueRepository::new(fixture.database.clone());

    let mut blocked_audio = item(
        "Audio",
        "Policy-blocked lyrics",
        Some(fixture.root_id),
        false,
    );
    blocked_audio.media_type = Some("Audio".to_owned());
    blocked_audio.data = Some(json!({
        "Lyrics": {
            "Metadata": {},
            "Lyrics": [{ "Text": "Private line", "Start": null, "Cues": null }]
        }
    }));
    let blocked_audio = items.create(blocked_audio).await.expect("blocked audio");
    values
        .link(
            blocked_audio.id,
            item_value::ItemValueType::Tags,
            "BlockedLyrics",
        )
        .await
        .expect("blocked lyric tag");

    let mut non_audio = item(
        "Movie",
        "Movie with lyric-shaped metadata",
        Some(fixture.root_id),
        false,
    );
    non_audio.media_type = Some("Video".to_owned());
    non_audio.data = Some(json!({
        "Lyrics": {
            "Metadata": {},
            "Lyrics": [{ "Text": "Not audio", "Start": null, "Cues": null }]
        }
    }));
    let non_audio = items.create(non_audio).await.expect("non-audio item");

    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.enable_lyric_management = true;
    policy.blocked_tags = vec!["BlockedLyrics".to_owned()];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("lyric manager policy");

    let blocked_base = format!("/Audio/{}/Lyrics", blocked_audio.id);
    for response in [
        request(&fixture.app, &blocked_base, &fixture.user_token).await,
        request(
            &fixture.app,
            &format!("/Audio/{}/RemoteSearch/Lyrics", blocked_audio.id),
            &fixture.user_token,
        )
        .await,
        request_post_body(
            &fixture.app,
            &format!("{blocked_base}?fileName=blocked.txt"),
            &fixture.user_token,
            "Replacement line",
        )
        .await,
        request_post(
            &fixture.app,
            &format!(
                "/Audio/{}/RemoteSearch/Lyrics/unavailable",
                blocked_audio.id
            ),
            &fixture.user_token,
        )
        .await,
        request_delete(&fixture.app, &blocked_base, &fixture.user_token).await,
    ] {
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let still_present = get_json(&fixture.app, &blocked_base, &fixture.administrator_token).await;
    assert_eq!(still_present["Lyrics"][0]["Text"], "Private line");

    let non_audio_response = request(
        &fixture.app,
        &format!("/Audio/{}/Lyrics", non_audio.id),
        &fixture.user_token,
    )
    .await;
    assert_eq!(non_audio_response.status(), StatusCode::NOT_FOUND);

    items
        .delete(blocked_audio.id)
        .await
        .expect("blocked audio cleanup");
    items.delete(non_audio.id).await.expect("non-audio cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn media_source_defaults_follow_target_user_stream_preferences() {
    let fixture = UserLibraryFixture::new().await;
    set_stream_preferences(&fixture.database, fixture.user_id).await;

    let items = BaseItemRepository::new(fixture.database.clone());
    let video = create_stream_defaults_video(&fixture, None).await;

    let route = format!(
        "/Users/{}/Items/{}?fields=MediaSources,MediaStreams",
        fixture.user_id, video.id
    );
    let item = get_json(&fixture.app, &route, &fixture.user_token).await;
    let source = &item["MediaSources"][0];
    assert_eq!(source["DefaultAudioStreamIndex"], 2);
    assert_eq!(source["DefaultSubtitleStreamIndex"], 3);

    let source_subtitle = stream_by_index(&source["MediaStreams"], 3);
    let top_level_subtitle = stream_by_index(&item["MediaStreams"], 3);
    assert_eq!(source_subtitle["SupportsExternalStream"], true);
    assert_eq!(top_level_subtitle["SupportsExternalStream"], true);
    assert_eq!(
        stream_by_index(&item["MediaStreams"], 0)["SupportsExternalStream"],
        false
    );
    assert_eq!(
        stream_by_index(&item["MediaStreams"], 1)["SupportsExternalStream"],
        false
    );
    assert!(source_subtitle["Score"].as_i64().is_some());
    assert_eq!(top_level_subtitle["Score"], source_subtitle["Score"]);
    assert!(
        stream_by_index(&item["MediaStreams"], 4)
            .get("Score")
            .is_none()
    );

    let mut invalid_remembered = NewUserData::new(video.id, fixture.user_id, video.id.to_string());
    invalid_remembered.audio_stream_index = Some(99);
    invalid_remembered.subtitle_stream_index = Some(99);
    UserDataRepository::new(fixture.database.clone())
        .upsert(invalid_remembered)
        .await
        .expect("invalid remembered streams");

    let item = get_json(&fixture.app, &route, &fixture.user_token).await;
    let source = &item["MediaSources"][0];
    assert_eq!(source["DefaultAudioStreamIndex"], 2);
    assert_eq!(source["DefaultSubtitleStreamIndex"], 3);

    let mut valid_remembered = NewUserData::new(video.id, fixture.user_id, video.id.to_string());
    valid_remembered.audio_stream_index = Some(1);
    valid_remembered.subtitle_stream_index = Some(4);
    UserDataRepository::new(fixture.database.clone())
        .upsert(valid_remembered)
        .await
        .expect("valid remembered streams");
    set_stream_preferences_with_remembering(&fixture.database, fixture.user_id, false).await;

    let item = get_json(&fixture.app, &route, &fixture.user_token).await;
    let source = &item["MediaSources"][0];
    assert_eq!(source["DefaultAudioStreamIndex"], 2);
    assert_eq!(source["DefaultSubtitleStreamIndex"], 3);

    set_stream_preferences_with_remembering(&fixture.database, fixture.user_id, true).await;
    let item = get_json(&fixture.app, &route, &fixture.user_token).await;
    let source = &item["MediaSources"][0];
    assert_eq!(source["DefaultAudioStreamIndex"], 1);
    assert_eq!(source["DefaultSubtitleStreamIndex"], 4);
    assert!(
        stream_by_index(&item["MediaStreams"], 4)
            .get("Score")
            .is_none()
    );

    items.delete(video.id).await.expect("video cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn media_sources_expand_all_video_versions_with_requested_version_first() {
    let fixture = UserLibraryFixture::new().await;
    set_original_language_preference(&fixture.database, fixture.user_id).await;
    let items = BaseItemRepository::new(fixture.database.clone());

    let mut primary = item("Movie", "Versioned Movie", Some(fixture.root_id), false);
    primary.media_type = Some("Video".to_owned());
    primary.path = Some("/media/versioned-movie-1080p.mkv".to_owned());
    primary.data = Some(json!({
        "Container": "mkv,webm",
        "OriginalLanguage": "English",
        "VideoType": "Dvd"
    }));
    let primary = items.create(primary).await.expect("primary version");
    let mut alternate = item("Movie", "Versioned Movie", Some(fixture.root_id), false);
    alternate.media_type = Some("Video".to_owned());
    alternate.path = Some("/media/versioned-movie-2160p.mkv".to_owned());
    alternate.primary_version_id = Some(primary.id);
    alternate.data = Some(json!({
        "Container": "mov,mkv",
        "OriginalLanguage": "French",
        "VideoType": "3"
    }));
    let alternate = items.create(alternate).await.expect("alternate version");

    for (source, codec, subtitle_codec) in [
        (&primary, "h264", "srt"),
        (&alternate, "hevc", "hdmv_pgs_subtitle"),
    ] {
        MediaStreamService::new(fixture.database.clone())
            .save_media_streams(
                source.id,
                vec![
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Video,
                        codec: Some(codec.to_owned()),
                        path: source.path.clone(),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("eng".to_owned()),
                        path: source.path.clone(),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("fre".to_owned()),
                        path: source.path.clone(),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 3,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some(subtitle_codec.to_owned()),
                        language: Some("eng".to_owned()),
                        path: source.path.clone(),
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("version streams");
    }

    let route = format!(
        "/Users/{}/Items/{}?Fields=MediaSources,MediaStreams",
        fixture.user_id, primary.id
    );
    let dto = get_json(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(dto["OriginalLanguage"], "English");
    assert_eq!(dto["Container"], "mkv,webm");
    let sources = dto["MediaSources"].as_array().expect("media sources");
    assert_eq!(sources.len(), 2);
    assert_eq!(dto["VideoType"], "Dvd");
    assert_eq!(sources[0]["Id"], primary.id.simple().to_string());
    assert_eq!(sources[0]["VideoType"], "Dvd");
    assert_eq!(sources[0]["Container"], "mkv");
    assert_eq!(sources[0]["Name"], "1080p");
    assert_eq!(sources[0]["DefaultAudioStreamIndex"], 1);
    assert_eq!(sources[1]["Name"], "2160p");
    assert_eq!(sources[1]["VideoType"], "BluRay");
    assert_eq!(sources[1]["Container"], "mkv");
    assert_eq!(sources[1]["DefaultAudioStreamIndex"], 2);
    assert_eq!(
        stream_by_index(&sources[0]["MediaStreams"], 3)["SupportsExternalStream"],
        true
    );
    assert_eq!(
        stream_by_index(&sources[1]["MediaStreams"], 3)["SupportsExternalStream"],
        true
    );
    assert_eq!(
        stream_by_index(&sources[1]["MediaStreams"], 0)["SupportsExternalStream"],
        false
    );
    assert_eq!(
        sources
            .iter()
            .map(|source| source["Id"].as_str().expect("source id"))
            .collect::<std::collections::HashSet<_>>(),
        [
            primary.id.simple().to_string(),
            alternate.id.simple().to_string()
        ]
        .iter()
        .map(String::as_str)
        .collect()
    );
    assert_eq!(dto["MediaStreams"], sources[0]["MediaStreams"]);
    assert_eq!(
        stream_by_index(&dto["MediaStreams"], 3)["SupportsExternalStream"],
        true
    );

    let alternate_dto = get_json(
        &fixture.app,
        &format!(
            "/Users/{}/Items/{}?Fields=MediaSources,MediaStreams",
            fixture.user_id, alternate.id
        ),
        &fixture.user_token,
    )
    .await;
    assert_eq!(alternate_dto["OriginalLanguage"], "French");
    assert_eq!(alternate_dto["Container"], "mov,mkv");
    assert_eq!(alternate_dto["VideoType"], "BluRay");
    assert_eq!(
        alternate_dto["MediaSources"][0]["Id"],
        alternate.id.simple().to_string()
    );
    assert_eq!(alternate_dto["MediaSources"][0]["VideoType"], "BluRay");
    assert_eq!(alternate_dto["MediaSources"][1]["VideoType"], "Dvd");
    assert_eq!(alternate_dto["MediaSources"][0]["Container"], "mkv");
    assert_eq!(alternate_dto["MediaSources"][1]["Container"], "mkv");
    assert_eq!(
        alternate_dto["MediaSources"][0]["DefaultAudioStreamIndex"],
        2
    );
    assert_eq!(
        alternate_dto["MediaSources"][1]["DefaultAudioStreamIndex"],
        1
    );

    items.delete(alternate.id).await.expect("alternate cleanup");
    items.delete(primary.id).await.expect("primary cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn item_details_hide_policy_blocked_alternate_media_sources() {
    let fixture = UserLibraryFixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let marker = Uuid::new_v4().simple().to_string();
    let primary_path = format!("/media/{marker}-primary.mkv");
    let hidden_path = format!("/media/{marker}-private.mkv");

    let mut primary = item(
        "Movie",
        "Policy-filtered versions",
        Some(fixture.root_id),
        false,
    );
    primary.media_type = Some("Video".to_owned());
    primary.path = Some(primary_path.clone());
    let primary = items.create(primary).await.expect("primary version");

    let mut alternate = item(
        "Movie",
        "Policy-filtered versions",
        Some(fixture.root_id),
        false,
    );
    alternate.media_type = Some("Video".to_owned());
    alternate.path = Some(hidden_path.clone());
    alternate.primary_version_id = Some(primary.id);
    let alternate = items.create(alternate).await.expect("private alternate");

    let streams = MediaStreamService::new(fixture.database.clone());
    for (source, path, codec) in [
        (&primary, primary_path.as_str(), "h264"),
        (&alternate, hidden_path.as_str(), "private-hevc"),
    ] {
        streams
            .save_media_streams(
                source.id,
                vec![MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some(codec.to_owned()),
                    path: Some(path.to_owned()),
                    ..MediaStream::default()
                }],
            )
            .await
            .expect("version stream");
    }
    MediaAttachmentService::new(fixture.database.clone())
        .save_media_attachments(
            alternate.id,
            vec![MediaAttachment {
                index: 1,
                file_name: Some("private-attachment.jpg".to_owned()),
                ..MediaAttachment::default()
            }],
        )
        .await
        .expect("private alternate attachment");
    ItemValueRepository::new(fixture.database.clone())
        .link(
            alternate.id,
            item_value::ItemValueType::Tags,
            "PrivateVersion",
        )
        .await
        .expect("private alternate tag");

    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.blocked_tags = vec!["privateversion".to_owned()];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted user policy");

    for route in [
        format!("/Users/{}/Items/{}", fixture.user_id, primary.id),
        format!("/Items/{}?UserId={}", primary.id, fixture.user_id),
    ] {
        let dto = get_json(&fixture.app, &route, &fixture.user_token).await;
        let sources = dto["MediaSources"].as_array().expect("media sources");
        assert_eq!(sources.len(), 1, "{route}");
        assert_eq!(sources[0]["Id"], primary.id.simple().to_string(), "{route}");
        assert!(dto.get("MediaSourceCount").is_none(), "{route}");
        let serialized = serde_json::to_string(&dto).unwrap();
        assert!(
            !serialized.contains(&alternate.id.simple().to_string()),
            "{route}"
        );
        assert!(!serialized.contains(&hidden_path), "{route}");
        assert!(!serialized.contains("private-hevc"), "{route}");
        assert!(!serialized.contains("private-attachment.jpg"), "{route}");
    }

    let admin_dto = get_json(
        &fixture.app,
        &format!("/Items/{}?UserId={}", primary.id, fixture.administrator_id),
        &fixture.administrator_token,
    )
    .await;
    assert_eq!(admin_dto["MediaSources"].as_array().unwrap().len(), 2);
    assert_eq!(admin_dto["MediaSourceCount"], 2);

    items.delete(alternate.id).await.expect("alternate cleanup");
    items.delete(primary.id).await.expect("primary cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn original_language_audio_preference_uses_item_metadata() {
    let fixture = UserLibraryFixture::new().await;
    set_original_language_preference(&fixture.database, fixture.user_id).await;

    let items = BaseItemRepository::new(fixture.database.clone());
    let video = create_stream_defaults_video(&fixture, Some("French")).await;
    for route in [
        format!(
            "/Users/{}/Items/{}?fields=MediaSources,MediaStreams",
            fixture.user_id, video.id
        ),
        format!(
            "/Items/{}?userId={}&fields=MediaSources,MediaStreams",
            video.id, fixture.user_id
        ),
    ] {
        let item = get_json(&fixture.app, &route, &fixture.user_token).await;
        assert_eq!(item["OriginalLanguage"], "French", "{route}");
        assert_eq!(
            item["MediaSources"][0]["DefaultAudioStreamIndex"], 1,
            "{route}"
        );
    }

    let page = get_json(
        &fixture.app,
        &format!(
            "/Items?userId={}&parentId={}&includeItemTypes=Movie",
            fixture.user_id, fixture.root_id
        ),
        &fixture.user_token,
    )
    .await;
    let listed = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == video.id.simple().to_string())
        .expect("original-language video in item page");
    assert_eq!(listed["OriginalLanguage"], "French");

    items.delete(video.id).await.expect("video cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn authentication_self_admin_and_current_routes_are_enforced() {
    let fixture = UserLibraryFixture::new().await;
    let legacy_routes = [
        format!("/Users/{}/Items/Root", fixture.administrator_id),
        format!(
            "/Users/{}/Items/{}",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/Intros",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/LocalTrailers",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/SpecialFeatures",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/Lyrics",
            fixture.administrator_id, fixture.item_id
        ),
    ];
    for route in &legacy_routes {
        let response = fixture
            .app
            .clone()
            .oneshot(Request::get(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{route}");

        let response = request(&fixture.app, route, &fixture.user_token).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{route}");

        let response = request(&fixture.app, route, &fixture.administrator_token).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
    }

    for route in [
        "/Items/Root".to_owned(),
        format!("/Items/{}", fixture.item_id),
        format!("/Items/{}/Intros", fixture.item_id),
        format!("/Items/{}/LocalTrailers", fixture.item_id),
        format!("/Items/{}/SpecialFeatures", fixture.item_id),
        format!("/Audio/{}/Lyrics", fixture.item_id),
    ] {
        let response = request(&fixture.app, &route, &fixture.user_token).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
    }

    let admin_for_user = format!("/Items/Root?userId={}", fixture.user_id);
    let response = request(&fixture.app, &admin_for_user, &fixture.administrator_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let regular_for_admin = format!("/Items/Root?userId={}", fixture.administrator_id);
    let response = request(&fixture.app, &regular_for_admin, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    fixture.cleanup().await;
}

#[tokio::test]
async fn concurrent_initialization_converges_on_one_postgres_root() {
    let database = test_database().await;
    let first = BaseItemRepository::new(database.clone());
    let second = BaseItemRepository::new(database.clone());
    let third = BaseItemRepository::new(database.clone());
    let (first, second, third) = tokio::join!(
        first.ensure_user_root(),
        second.ensure_user_root(),
        third.ensure_user_root()
    );
    assert_eq!(first.unwrap().id, USER_ROOT_FOLDER_ID);
    assert_eq!(second.unwrap().id, USER_ROOT_FOLDER_ID);
    assert_eq!(third.unwrap().id, USER_ROOT_FOLDER_ID);
    let root_count = base_item::Entity::find()
        .filter(base_item::Column::ItemType.eq("UserRootFolder"))
        .count(&database)
        .await
        .expect("root count");
    assert_eq!(root_count, 1);
}

struct UserLibraryFixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    root_id: Uuid,
    item_id: Uuid,
    intro_id: Uuid,
    trailer_id: Uuid,
    feature_id: Uuid,
    storage_root: PathBuf,
}

impl UserLibraryFixture {
    async fn new() -> Self {
        Self::with_lyric_providers(Vec::new()).await
    }

    async fn with_lyric_providers(providers: Vec<Arc<dyn LyricProvider>>) -> Self {
        let database = test_database().await;
        let users = UserService::new(database.clone());
        let suffix = Uuid::new_v4().simple().to_string();
        let storage_root = std::env::temp_dir().join(format!("jellyfin-user-library-{suffix}"));
        let administrator = users
            .create_initial_administrator(&format!("library-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("library-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let administrator_token = devices
            .create_session(NewDevice::new(
                administrator.id,
                "User Library Tests",
                "1.0",
                "Test",
                format!("library-admin-{suffix}"),
            ))
            .await
            .expect("administrator session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "User Library Tests",
                "1.0",
                "Test",
                format!("library-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let mut media = item("Audio", "Test Song", Some(root.id), false);
        media.media_type = Some("Audio".to_owned());
        media.path = Some("/media/Test Song.mkv".to_owned());
        media.data = Some(json!({
            "Lyrics": {
                "Metadata": { "Artist": "Test Artist" },
                "Lyrics": [{ "Text": "First line", "Start": 0, "Cues": null }]
            }
        }));
        let media = items.create(media).await.expect("media item");
        save_media_source_metadata(&database, media.id).await;

        let mut intro = item("Video", "Intro", Some(media.id), false);
        intro.data = Some(json!({ "IsIntro": true }));
        let intro = items.create(intro).await.expect("intro item");

        let nested = items
            .create(item("Folder", "Extras", Some(media.id), true))
            .await
            .expect("nested extras folder");
        let mut trailer = item("Video", "Trailer", Some(nested.id), false);
        trailer.data = Some(json!({ "ExtraType": "Trailer" }));
        let trailer = items.create(trailer).await.expect("trailer item");
        let mut feature = item("Video", "Feature", Some(media.id), false);
        feature.data = Some(json!({ "ExtraType": "Featurette" }));
        let feature = items.create(feature).await.expect("feature item");

        let state = AppState::new(
            database.clone(),
            "User Library Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            storage_root.join("programdata"),
            storage_root.join("web"),
            storage_root.join("cache/images"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
        )
        .with_lyric_providers(providers);
        let app = jellyfin_api::router(state);
        Self {
            database,
            app,
            administrator_id: administrator.id,
            administrator_token,
            user_id: user.id,
            user_token,
            root_id: root.id,
            item_id: media.id,
            intro_id: intro.id,
            trailer_id: trailer.id,
            feature_id: feature.id,
            storage_root,
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
        let _ = tokio::fs::remove_dir_all(self.storage_root).await;
    }
}

async fn save_media_source_metadata(database: &DatabaseConnection, item_id: Uuid) {
    MediaStreamService::new(database.clone())
        .save_media_streams(
            item_id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Audio,
                codec: Some("ac3".to_owned()),
                language: Some("ger".to_owned()),
                path: Some("/media/Test Song.mkv".to_owned()),
                is_default: true,
                ..MediaStream::default()
            }],
        )
        .await
        .expect("media streams");
    MediaAttachmentService::new(database.clone())
        .save_media_attachments(
            item_id,
            vec![MediaAttachment {
                index: 4,
                codec: Some("mjpeg".to_owned()),
                file_name: Some("poster.jpg".to_owned()),
                mime_type: Some("image/jpeg".to_owned()),
                ..MediaAttachment::default()
            }],
        )
        .await
        .expect("media attachments");
}

async fn set_stream_preferences(database: &DatabaseConnection, user_id: Uuid) {
    set_stream_preferences_with_remembering(database, user_id, true).await;
}

async fn set_original_language_preference(database: &DatabaseConnection, user_id: Uuid) {
    set_stream_preferences_with(
        database,
        user_id,
        "OriginalLanguage",
        false,
        "English",
        "Always",
        true,
    )
    .await;
}

async fn set_stream_preferences_with_remembering(
    database: &DatabaseConnection,
    user_id: Uuid,
    remember: bool,
) {
    set_stream_preferences_with(
        database, user_id, "English", false, "English", "Always", remember,
    )
    .await;
}

async fn set_stream_preferences_with(
    database: &DatabaseConnection,
    user_id: Uuid,
    audio_language: &str,
    play_default_audio_track: bool,
    subtitle_language: &str,
    subtitle_mode: &str,
    remember: bool,
) {
    user::ActiveModel {
        id: Set(user_id),
        preferences: Set(json!({
            "AudioLanguagePreference": audio_language,
            "PlayDefaultAudioTrack": play_default_audio_track,
            "SubtitleLanguagePreference": subtitle_language,
            "SubtitleMode": subtitle_mode,
            "RememberAudioSelections": remember,
            "RememberSubtitleSelections": remember,
            "EnableNextEpisodeAutoPlay": true
        })),
        ..Default::default()
    }
    .update(database)
    .await
    .expect("stream preference update");
}

fn stream_by_index(streams: &Value, index: i64) -> &Value {
    streams
        .as_array()
        .expect("media streams array")
        .iter()
        .find(|stream| stream["Index"] == index)
        .expect("media stream index")
}

async fn create_stream_defaults_video(
    fixture: &UserLibraryFixture,
    original_language: Option<&str>,
) -> jellyfin_data::entities::base_item::Model {
    let path = format!("/media/Stream Defaults {}.mkv", Uuid::new_v4().simple());
    let items = BaseItemRepository::new(fixture.database.clone());
    let mut video = item("Movie", "Stream Defaults", Some(fixture.root_id), false);
    video.media_type = Some("Video".to_owned());
    video.path = Some(path.clone());
    if let Some(original_language) = original_language {
        video.data = Some(json!({ "OriginalLanguage": original_language }));
    }
    let video = items.create(video).await.expect("video item");
    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            video.id,
            vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("ac3".to_owned()),
                    language: Some("fre".to_owned()),
                    is_default: true,
                    is_original: true,
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 2,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("aac".to_owned()),
                    language: Some("eng".to_owned()),
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 3,
                    stream_type: MediaStreamType::Subtitle,
                    codec: Some("srt".to_owned()),
                    language: Some("eng".to_owned()),
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 4,
                    stream_type: MediaStreamType::Subtitle,
                    codec: Some("srt".to_owned()),
                    language: Some("eng".to_owned()),
                    is_forced: true,
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 5,
                    stream_type: MediaStreamType::Subtitle,
                    codec: Some("srt".to_owned()),
                    language: Some("fre".to_owned()),
                    is_forced: true,
                    path: Some(path),
                    ..MediaStream::default()
                },
            ],
        )
        .await
        .expect("video streams");
    video
}

fn item(item_type: &str, name: &str, parent_id: Option<Uuid>, is_folder: bool) -> NewBaseItem {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item
}

fn assert_base_item(body: &Value, id: Uuid, item_type: &str, name: &str) {
    assert_eq!(body["Id"], id.simple().to_string());
    assert_eq!(body["Type"], item_type);
    assert_eq!(body["Name"], name);
    assert_eq!(body["ServerId"].as_str().unwrap().len(), 32);
    assert!(body["DateCreated"].is_string());
    assert!(body["Etag"].is_string());
}

async fn request(app: &axum::Router, uri: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(uri)
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

async fn request_post(app: &axum::Router, uri: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
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

async fn request_post_body(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn request_post_bytes(
    app: &axum::Router,
    uri: &str,
    token: &str,
    body: Vec<u8>,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(uri)
                .header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn utf16_lyric_bytes(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xFF, 0xFE]
    } else {
        vec![0xFE, 0xFF]
    };
    bytes.extend(text.encode_utf16().flat_map(|unit| {
        if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        }
    }));
    bytes
}

fn utf32_lyric_bytes(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xFF, 0xFE, 0x00, 0x00]
    } else {
        vec![0x00, 0x00, 0xFE, 0xFF]
    };
    bytes.extend(text.chars().map(u32::from).flat_map(|unit| {
        if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        }
    }));
    bytes
}

async fn request_delete(app: &axum::Router, uri: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::delete(uri)
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

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let response = request(app, uri, token).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    body_json(response).await
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
