use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::MediaAttachmentService;
use jellyfin_controller::MediaStreamService;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, NewUserData, USER_ROOT_FOLDER_ID,
    UserDataRepository,
    entities::{base_item, user},
};
use jellyfin_model::{MediaAttachment, MediaStream, MediaStreamType};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Set,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User Library Tests\", Device=\"Test\", DeviceId=\"user-library\", Version=\"1.0\"";

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
    assert_eq!(item["HasLyrics"], true);
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
async fn media_stream_fields_are_projected_for_single_item_routes() {
    let fixture = UserLibraryFixture::new().await;

    let route = format!(
        "/Users/{}/Items/{}?fields=MediaSources,MediaStreams",
        fixture.user_id, fixture.item_id
    );
    let item = get_json(&fixture.app, &route, &fixture.user_token).await;

    assert_eq!(item["MediaSources"].as_array().unwrap().len(), 1);
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
    assert_eq!(item["MediaStreams"][0]["Language"], "deu");

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
async fn original_language_audio_preference_uses_item_metadata() {
    let fixture = UserLibraryFixture::new().await;
    set_original_language_preference(&fixture.database, fixture.user_id).await;

    let items = BaseItemRepository::new(fixture.database.clone());
    let video = create_stream_defaults_video(&fixture, Some("French")).await;
    let route = format!(
        "/Users/{}/Items/{}?fields=MediaSources,MediaStreams",
        fixture.user_id, video.id
    );

    let item = get_json(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(item["MediaSources"][0]["DefaultAudioStreamIndex"], 1);

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
}

impl UserLibraryFixture {
    async fn new() -> Self {
        let database = test_database().await;
        let users = UserService::new(database.clone());
        let suffix = Uuid::new_v4().simple().to_string();
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

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "User Library Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
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
    }
}

async fn save_media_source_metadata(database: &DatabaseConnection, item_id: Uuid) {
    MediaStreamService::new(database.clone())
        .save_media_streams(
            item_id,
            &[MediaStream {
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
            &[MediaAttachment {
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
            &[
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

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let response = request(app, uri, token).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
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
