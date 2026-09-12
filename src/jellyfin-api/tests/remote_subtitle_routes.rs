use std::sync::{Arc, Mutex};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{SubtitleProvider, SubtitleResponse, SubtitleSearchRequest, UserService};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, MediaStreamQuery, MediaStreamRepository,
    NewBaseItem, NewDevice, PersistedMediaStreamType,
};
use jellyfin_model::RemoteSubtitleInfo;
use sea_orm::ConnectionTrait;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Remote Subtitle Tests\", Device=\"Test\", DeviceId=\"remote-subtitles\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_remote_subtitle_routes_";
const SUBTITLE: &[u8] = b"1\n00:00:01,000 --> 00:00:02,000\nHello from provider\n";

struct TestSubtitleProvider {
    searches: Arc<Mutex<Vec<SubtitleSearchRequest>>>,
}

impl SubtitleProvider for TestSubtitleProvider {
    fn name(&self) -> &'static str {
        "TestSubtitles"
    }

    fn supported_media_types(&self) -> &[&str] {
        &["Movie", "Episode"]
    }

    fn search(&self, request: &SubtitleSearchRequest) -> Vec<RemoteSubtitleInfo> {
        self.searches.lock().unwrap().push(request.clone());
        Vec::new()
    }

    fn get_subtitles(&self, id: &str) -> Option<SubtitleResponse> {
        (id == "TestSubtitles_eng").then(|| SubtitleResponse {
            format: "srt".to_owned(),
            language: "eng".to_owned(),
            content: SUBTITLE.to_vec(),
            is_forced: true,
            is_hearing_impaired: false,
        })
    }
}

#[tokio::test]
async fn remote_subtitle_preview_streams_bytes_and_download_registers_the_stream() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise_routes(&task_database_name).await }).await;

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

async fn exercise_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .unwrap();
    jellyfin_data::migrate(&database).await.unwrap();

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let admin = users
        .create_initial_administrator(&format!("subtitle-admin-{suffix}"))
        .await
        .unwrap();
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            admin.id,
            "Remote Subtitle Tests",
            "1.0",
            "Test",
            format!("remote-subtitle-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token;
    let storage_root = std::env::temp_dir().join(format!("jellyfin-remote-subtitle-{suffix}"));
    let media_path = storage_root.join("Movie.mkv");
    std::fs::create_dir_all(&storage_root).unwrap();
    std::fs::write(&media_path, b"video").unwrap();
    let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    movie.name = Some("Remote Subtitle Movie".to_owned());
    movie.media_type = Some("Video".to_owned());
    movie.path = Some(media_path.to_string_lossy().into_owned());
    let movie = BaseItemRepository::new(database.clone())
        .create(movie)
        .await
        .unwrap();
    let mut series = NewBaseItem::new(Uuid::new_v4(), "Series");
    series.name = Some("Linked Series".to_owned());
    series.is_folder = true;
    let series = BaseItemRepository::new(database.clone())
        .create(series)
        .await
        .unwrap();
    let mut episode = NewBaseItem::new(Uuid::new_v4(), "Episode");
    episode.name = Some("Linked Episode".to_owned());
    episode.media_type = Some("Video".to_owned());
    episode.path = Some(media_path.to_string_lossy().into_owned());
    episode.parent_id = Some(series.id);
    episode.series_id = Some(series.id);
    let episode = BaseItemRepository::new(database.clone())
        .create(episode)
        .await
        .unwrap();
    let searches = Arc::new(Mutex::new(Vec::new()));
    let app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "Remote Subtitle Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            storage_root.join("programdata"),
            storage_root.join("web"),
            storage_root.join("images"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
        )
        .with_subtitle_providers(vec![Arc::new(TestSubtitleProvider {
            searches: Arc::clone(&searches),
        })]),
    );

    let preview = request(
        &app,
        "/providers/subtitles/subtitles/TestSubtitles_eng",
        "GET",
        &token,
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert!(preview.headers().contains_key(header::CONTENT_TYPE));
    assert_eq!(
        to_bytes(preview.into_body(), usize::MAX).await.unwrap(),
        SUBTITLE
    );

    let missing = request(
        &app,
        "/Providers/Subtitles/Subtitles/Unknown_eng",
        "GET",
        &token,
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let download = request(
        &app,
        &format!(
            "/items/{}/remotesearch/subtitles/TestSubtitles_eng",
            movie.id
        ),
        "POST",
        &token,
    )
    .await;
    assert_eq!(download.status(), StatusCode::NO_CONTENT);

    let streams = MediaStreamRepository::new(database.clone())
        .query(MediaStreamQuery {
            item_id: movie.id,
            stream_index: None,
            stream_type: Some(PersistedMediaStreamType::Subtitle),
        })
        .await
        .unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].codec.as_deref(), Some("srt"));
    assert_eq!(streams[0].language.as_deref(), Some("eng"));
    assert!(streams[0].is_forced);
    assert_eq!(
        std::fs::read(streams[0].path.as_deref().unwrap()).unwrap(),
        SUBTITLE
    );

    let search = request(
        &app,
        &format!(
            "/items/{}/remotesearch/subtitles/eng?isperfectmatch=true",
            episode.id
        ),
        "GET",
        &token,
    )
    .await;
    assert_eq!(search.status(), StatusCode::OK);
    {
        let searches = searches.lock().unwrap();
        assert_eq!(searches.len(), 1);
        assert_eq!(searches[0].series_name.as_deref(), Some("Linked Series"));
        assert!(searches[0].is_perfect_match);
    }

    database.close().await.unwrap();
    std::fs::remove_dir_all(storage_root).unwrap();
}

async fn request(
    app: &axum::Router,
    uri: &str,
    method: &str,
    token: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
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
