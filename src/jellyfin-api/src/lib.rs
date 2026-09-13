use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use futures_util::{StreamExt, stream};
use jellyfin_controller::{
    ArtistError, ArtistService, ChapterImageService, CollectionError, CollectionService,
    DashboardError, DashboardPage, DashboardService, EnvironmentError, EnvironmentService,
    GameGenreError, GameGenreService, GenreError, GenreService, InstalledPlugin, ItemByNameService,
    ItemImageError, ItemImageService, ItemLookupError, ItemLookupService, ItemUpdateError,
    ItemUpdateService, LibraryControllerError, LibraryControllerService, LibraryScanError,
    LibraryScanService, LiveStreamRegistry, LocalizationService, MediaAttachmentService,
    MediaAttachmentServiceError, MediaSegmentError, MediaSegmentManagerService, MediaStreamService,
    MediaStreamServiceError, MetadataEditorError, MetadataEditorService, MetadataRefreshMode,
    MetadataRefreshOptions, MetadataRefreshService, MusicGenreError, MusicGenreService,
    PackageError, PackageService, PersonError, PersonService, PlaylistError, PlaylistService,
    PlaystateError, PlaystateService, PluginRegistry, PostgresSessionStore, ScheduledTaskError,
    ScheduledTaskService, SearchManager, SearchProvider, StudioError, StudioService,
    SubtitleManager, SubtitleProvider, SystemLogError, SystemLogService, SystemStorageService,
    TranscodeJobRegistry, TrickplayError, TrickplayService, UserCopyOptions, UserDataService,
    UserDataServiceError, UserError, UserLibraryError, UserLibraryService, UserService,
    UserViewManagerError, UserViewManagerService, VideoError, VideoService, VirtualFolderService,
    VirtualFolderServiceError, YearError, YearService, client_event::ClientEventLogger,
};
use jellyfin_data::{
    ActivityLogError, ActivityLogRepository, ApiKeyRepository, AuthenticationStoreError,
    BaseItemError, BaseItemImageRepository, BaseItemRepository, ChapterRepository,
    DeviceOptionsRepository, DeviceRepository, DisplayPreferenceRepository,
    DisplayPreferenceStoreError, EmbyItemAccessLevel, EmbyItemAccessRepository,
    EmbyItemAccessStoreError, ItemUpdateRepository, ItemUpdateStoreError, ItemValueRepository,
    KeyframeDataRepository, NamedConfigurationRepository, NamedConfigurationStoreError,
    PersonRepository, QuickConnectRepository, RememberedTrackSelection,
    ServerConfigurationRepository, ServerConfigurationStoreError, SessionCommandRepository,
    SessionCommandStoreError, UserDataRepository, UserSearchStateRepository,
    entities::{user, user_profile_image},
};
use jellyfin_drawing::{ImageProcessingError, ImageProcessor};
use jellyfin_live_tv::{
    listings::{GuideRefreshError, GuideRefreshService},
    tuner_hosts::{TunerHostError, TunerHostManager},
};
use jellyfin_media_encoding::encoder::EncoderCapabilities;
use jellyfin_model::{
    DisplayPreferencesDto, FileSystemEntryInfo, PublicSystemInfo, SystemInfo, TranscodeReason,
    UserConfiguration, UserDto, UserItemDataDto, UserPolicy,
};
use jellyfin_networking::{NetworkConfiguration, NetworkManager};
use jellyfin_server_implementations::{
    AuthenticationError, DefaultAuthenticationProvider, PersistedDtoImageProjectionService,
    QuickConnectError, QuickConnectManager, SessionManager, SyncPlayManager,
    SystemQuickConnectCapability,
};
use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

/// Official Jellyfin API version implemented by the checked-in reference tree.
///
/// This must not use the Rust crate version: clients compare this field with
/// their minimum supported Jellyfin server version before issuing home-page
/// requests.
const JELLYFIN_API_VERSION: &str = "12.0.0";

mod activity_log;
mod api_keys;
mod artists;
mod audio;
mod authentication;
mod authorization;
pub mod backup;
mod branding;
mod channels;
mod client_log;
mod collections;
mod configuration;
mod dashboard;
mod devices;
mod display_preferences;
mod encoding_runtime;
mod environment;
mod filters;
mod game_genre;
mod genres;
mod hls_segment;
mod item_images;
mod item_lookup;
mod item_refresh;
mod item_update;
mod items;
mod library;
mod live_tv;
mod localization;
mod media_info;
mod media_segments;
mod media_source;
mod movies;
mod music_genre;
mod openapi;
mod packages;
mod persons;
mod playlists;
mod playstate;
mod plugins;
pub mod query;
mod quick_connect;
mod remote_images;
mod robots;
mod scheduled_tasks;
mod search;
mod session;
mod startup;
mod stream_options;
mod studios;
mod subtitles;
mod sync_play;
mod system;
mod time_sync;
mod trailers;
mod trickplay;
mod tv_shows;
mod user_data;
mod user_library;
mod user_views;
mod users;
mod video_attachments;
mod videos;
mod virtual_folders;
mod websocket;
mod years;

pub use backup::restore_backup_at_startup;
pub use branding::BrandingOptions;
pub use subtitles::emby_legacy_subtitle_delete_routes;
pub use system::emby_log_file_lines;

/// Emby-only legacy GameGenre routes. These are deliberately not merged into
/// [`unprefixed_router`].
pub fn emby_game_genre_routes() -> Router<Arc<AppState>> {
    game_genre::routes()
}

/// Emby-only legacy Game routes. These reuse the shared policy-aware
/// implementation without adding removed Game endpoints to Jellyfin.
pub fn emby_game_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Games/{item_id}/Similar", get(library::emby_game_similar))
        .route("/games/{item_id}/similar", get(library::emby_game_similar))
        .route(
            "/Items/RemoteSearch/Game",
            post(item_lookup::emby_game_remote_search),
        )
        .route(
            "/items/remotesearch/game",
            post(item_lookup::emby_game_remote_search),
        )
}

/// Host lifecycle commands exposed by the system API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemCommand {
    Restart,
    Shutdown,
    /// Restart the server and restore the validated archive before serving requests.
    Restore(PathBuf),
}

const EMBY_METADATA_REFRESH_QUEUE_CAPACITY: usize = 64;
const EMBY_METADATA_REFRESH_CONCURRENCY: usize = 4;
const EMBY_METADATA_REFRESH_CHUNK_SIZE: usize = 128;

struct QueuedEmbyMetadataRefresh {
    item_ids: Vec<Uuid>,
    tmdb_api_key: Arc<str>,
    omdb_api_key: Arc<str>,
}

fn start_emby_metadata_refresh_worker(
    service: MetadataRefreshService,
) -> tokio::sync::mpsc::Sender<QueuedEmbyMetadataRefresh> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<QueuedEmbyMetadataRefresh>(
        EMBY_METADATA_REFRESH_QUEUE_CAPACITY,
    );
    tokio::spawn(async move {
        // `recv` continues yielding already-buffered chunks after the final
        // AppState sender is dropped, so normal server teardown drains all
        // accepted work before this worker exits. An externally forced Tokio
        // runtime stop, like process termination, cannot provide that grace.
        while let Some(batch) = receiver.recv().await {
            debug_assert!(batch.item_ids.len() <= EMBY_METADATA_REFRESH_CHUNK_SIZE);
            stream::iter(batch.item_ids)
                .for_each_concurrent(EMBY_METADATA_REFRESH_CONCURRENCY, |item_id| {
                    let service = service.clone();
                    let tmdb_api_key = Arc::clone(&batch.tmdb_api_key);
                    let omdb_api_key = Arc::clone(&batch.omdb_api_key);
                    async move {
                        if let Err(error) = service
                            .refresh(
                                item_id,
                                &tmdb_api_key,
                                &omdb_api_key,
                                MetadataRefreshOptions {
                                    metadata_refresh_mode: MetadataRefreshMode::FullRefresh,
                                    image_refresh_mode: MetadataRefreshMode::None,
                                    replace_all_metadata: true,
                                    replace_all_images: false,
                                },
                            )
                            .await
                        {
                            tracing::warn!(%error, %item_id, "queued Emby metadata reset refresh failed");
                        }
                    }
                })
                .await;
        }
    });
    sender
}

/// Applies the shared route authorization policy to another protocol's route
/// tree. Protocol crates can own their wire contracts without duplicating
/// token, API-key, and user-policy checks.
pub async fn protocol_route_auth(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match authorization::require_route_auth(State(state), request, next).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) users: UserService,
    pub(crate) activity_logs: ActivityLogRepository,
    pub(crate) api_keys: ApiKeyRepository,
    pub(crate) devices: DeviceRepository,
    pub(crate) device_options: DeviceOptionsRepository,
    pub(crate) display_preferences: DisplayPreferenceRepository,
    pub(crate) session_commands: SessionCommandRepository,
    pub(crate) sync_play: SyncPlayManager,
    pub(crate) web_sockets: Arc<websocket::WebSocketHub>,
    pub(crate) quick_connect:
        QuickConnectManager<Arc<jellyfin_server_implementations::SystemQuickConnectCapability>>,
    pub(crate) quick_connect_capability: Arc<SystemQuickConnectCapability>,
    pub(crate) playstate: PlaystateService,
    pub(crate) playlists: PlaylistService,
    pub(crate) collections: CollectionService,
    pub(crate) user_data: UserDataService,
    pub(crate) artists: ArtistService,
    pub(crate) genres: GenreService,
    pub(crate) game_genres: GameGenreService,
    pub(crate) studios: StudioService,
    pub(crate) music_genres: MusicGenreService,
    pub(crate) persons: PersonService,
    pub(crate) item_images: Arc<ItemImageService>,
    pub(crate) metadata_refresh: MetadataRefreshService,
    emby_metadata_refresh_sender: tokio::sync::mpsc::Sender<QueuedEmbyMetadataRefresh>,
    pub(crate) base_items: Arc<BaseItemRepository>,
    pub(crate) chapters: ChapterRepository,
    pub(crate) item_values: ItemValueRepository,
    pub(crate) people: PersonRepository,
    pub(crate) dto_images: PersistedDtoImageProjectionService<Arc<ItemImageService>>,
    pub(crate) image_processor: ImageProcessor,
    pub(crate) item_lookup: ItemLookupService,
    pub(crate) item_update: ItemUpdateService,
    pub(crate) metadata_editor: MetadataEditorService,
    pub(crate) localization: LocalizationService,
    pub(crate) server_configuration: ServerConfigurationRepository,
    pub(crate) user_library: UserLibraryService,
    pub(crate) search: SearchManager,
    pub(crate) library_controller: LibraryControllerService,
    pub(crate) media_attachments: MediaAttachmentService,
    pub(crate) media_segments: MediaSegmentManagerService,
    pub(crate) media_streams: MediaStreamService,
    pub(crate) subtitles: SubtitleManager,
    pub(crate) videos: VideoService,
    pub(crate) years: YearService,
    pub(crate) tuner_hosts: TunerHostManager,
    pub(crate) live_tv_guide: Option<Arc<GuideRefreshService>>,
    pub(crate) virtual_folders: Arc<VirtualFolderService>,
    pub(crate) user_views: UserViewManagerService,
    pub(crate) dashboard: DashboardService,
    pub(crate) environment: EnvironmentService,
    pub(crate) plugins: PluginRegistry,
    pub(crate) packages: PackageService,
    pub(crate) scheduled_tasks: ScheduledTaskService,
    pub(crate) library_scan: Arc<LibraryScanService>,
    pub(crate) system_logs: SystemLogService,
    pub(crate) system_storage: SystemStorageService,
    pub(crate) trickplay: Arc<TrickplayService>,
    pub(crate) client_event_logger: ClientEventLogger,
    pub(crate) named_configurations: Option<NamedConfigurationRepository>,
    pub(crate) program_data_directory: PathBuf,
    pub(crate) web_directory: PathBuf,
    pub(crate) image_cache_directory: PathBuf,
    pub(crate) cache_directory: PathBuf,
    pub(crate) internal_metadata_directory: PathBuf,
    pub(crate) network_manager: Arc<NetworkManager>,
    pub(crate) remote_stream_client: reqwest::Client,
    pub(crate) transcode_directory: Arc<std::path::Path>,
    pub(crate) ffmpeg_path: Arc<PathBuf>,
    pub(crate) encoder_capabilities: EncoderCapabilities,
    pub(crate) live_streams: LiveStreamRegistry,
    pub(crate) transcode_jobs: Arc<TranscodeJobRegistry>,
    pub(crate) authentication: DefaultAuthenticationProvider,
    pub(crate) session_manager: SessionManager<PostgresSessionStore>,
    pub(crate) branding: Arc<tokio::sync::RwLock<BrandingOptions>>,
    pub(crate) system_info: PublicSystemInfo,
    pub(crate) startup: Arc<Mutex<startup::StartupState>>,
    pub(crate) startup_repository: Option<ServerConfigurationRepository>,
    pub(crate) database: jellyfin_data::SharedDatabase,
    pub(crate) tmdb_api_key: Arc<tokio::sync::RwLock<Arc<str>>>,
    pub(crate) omdb_api_key: Arc<tokio::sync::RwLock<Arc<str>>>,
    pub(crate) system_command: Arc<dyn Fn(SystemCommand) + Send + Sync>,
    pub(crate) metrics_enabled: Arc<std::sync::atomic::AtomicBool>,
}

/// Protocol adapter selection for Emby's independently copyable user state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmbyUserCopyOptions {
    pub policy: bool,
    pub configuration: bool,
    pub user_data: bool,
}

/// Parsed fields for Emby's protocol-private item-access mutation.
#[derive(Debug)]
pub struct EmbyItemAccessMutation {
    pub item_ids: Option<Vec<String>>,
    pub user_ids: Option<Vec<String>>,
    /// `None` represents both an omitted/null field and Emby's numeric `None`
    /// value; all three remove an explicit assignment.
    pub access_level: Option<i16>,
}

/// Protocol-neutral values used by Emby's legacy Person credits adapter.
///
/// The generated Emby response is owned by `jellyfin-emby-api`; this record
/// only carries policy-filtered PostgreSQL values across the crate boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbyPersonCreditRecord {
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub provider_ids: HashMap<String, String>,
    pub production_year: Option<i32>,
    pub index_number: Option<i32>,
    pub index_number_end: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub premiere_date: Option<String>,
    pub person_type: String,
    pub role: Option<String>,
    pub item_type: String,
    pub overview: Option<String>,
}

/// Parsed fields for Emby's protocol-private shared-item leave mutation.
#[derive(Debug)]
pub struct EmbyLeaveSharedItemsMutation {
    pub item_ids: Option<Vec<String>>,
    pub user_id: Option<String>,
}

/// Authenticated request-session snapshot used by Emby's process-local party
/// service. API keys authenticate successfully but have no associated user,
/// matching Emby's synthetic user-less session context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbyPartySessionContext {
    pub session_id: String,
    pub user: Option<EmbyPartyUser>,
    pub has_now_playing_item: bool,
}

/// Minimal user shape serialized inside Emby's legacy `PartySessionInfo` and
/// `PartyMessageDto` objects.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EmbyPartyUser {
    pub id: String,
    pub name: String,
}

impl From<EmbyUserCopyOptions> for UserCopyOptions {
    fn from(value: EmbyUserCopyOptions) -> Self {
        Self {
            policy: value.policy,
            configuration: value.configuration,
            user_data: value.user_data,
        }
    }
}

impl AppState {
    /// Returns the encoder snapshot captured during server startup.
    ///
    /// Protocol adapters use this rather than inventing a static codec list.
    #[must_use]
    pub fn encoder_codec_names(&self) -> (Vec<String>, Vec<String>) {
        (
            self.encoder_capabilities.encoders.clone(),
            self.encoder_capabilities.decoders.clone(),
        )
    }

    /// Executes a protocol adapter's directory request through the shared
    /// Jellyfin environment service.
    pub fn environment_directory_contents(
        &self,
        path: &str,
        include_files: bool,
        include_directories: bool,
    ) -> Result<Vec<FileSystemEntryInfo>, Response> {
        self.environment
            .directory_contents(path, include_files, include_directories)
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }

    /// Returns the server's current filesystem roots for protocol adapters.
    #[must_use]
    pub fn environment_drives(&self) -> Vec<FileSystemEntryInfo> {
        self.environment.drives()
    }

    /// Resolves a path through the shared platform-aware parent-path logic.
    #[must_use]
    pub fn environment_parent_path(&self, path: &str) -> Option<String> {
        self.environment.parent_path(path)
    }

    /// Validates a protocol adapter's filesystem path request.
    pub fn environment_validate_path(
        &self,
        path: Option<&str>,
        is_file: Option<bool>,
        validate_writable: bool,
    ) -> Result<(), Response> {
        self.environment
            .validate_path(path, is_file, validate_writable)
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }

    #[allow(clippy::too_many_lines)]
    pub fn new(
        database: impl Into<jellyfin_data::SharedDatabase>,
        server_name: String,
        local_address: String,
    ) -> Self {
        let database = database.into();
        let library_scan = Arc::new(LibraryScanService::new(Arc::clone(&database)));
        let scheduled_tasks =
            ScheduledTaskService::with_default_executors(Arc::clone(&library_scan));
        let trickplay = Arc::new(TrickplayService::new(
            Arc::clone(&database),
            PathBuf::from("programdata").join("trickplay"),
        ));
        let chapter_images = ChapterImageService::new(
            Arc::clone(&database),
            PathBuf::from("programdata").join("chapter-images"),
            PathBuf::from("ffmpeg"),
        );
        let item_images = Arc::new(ItemImageService::new(Arc::clone(&database)));
        let metadata_refresh =
            MetadataRefreshService::new(Arc::clone(&database), Some(Arc::clone(&item_images)));
        let emby_metadata_refresh_sender =
            start_emby_metadata_refresh_worker(metadata_refresh.clone());
        let base_items = Arc::new(BaseItemRepository::new(Arc::clone(&database)));
        let item_values = ItemValueRepository::new(Arc::clone(&database));
        let people = PersonRepository::new(Arc::clone(&database));
        let web_sockets = Arc::new(websocket::WebSocketHub::new());
        let quick_connect_capability = Arc::new(SystemQuickConnectCapability::new(true));
        let user_library = UserLibraryService::new(Arc::clone(&database));
        let item_by_name = ItemByNameService::new(Arc::clone(&database));
        let search = SearchManager::with_default_database(Arc::clone(&database));
        let session_store = PostgresSessionStore::new(
            UserService::new(Arc::clone(&database)),
            DeviceRepository::new(Arc::clone(&database)),
            ActivityLogRepository::new(Arc::clone(&database)),
            DefaultAuthenticationProvider::new(),
        );
        let state = Self {
            users: UserService::new(Arc::clone(&database)),
            activity_logs: ActivityLogRepository::new(Arc::clone(&database)),
            api_keys: ApiKeyRepository::new(Arc::clone(&database)),
            devices: DeviceRepository::new(Arc::clone(&database)),
            device_options: DeviceOptionsRepository::new(Arc::clone(&database)),
            display_preferences: DisplayPreferenceRepository::new(Arc::clone(&database)),
            session_commands: SessionCommandRepository::new(Arc::clone(&database)),
            sync_play: SyncPlayManager::new(),
            web_sockets: Arc::clone(&web_sockets),
            quick_connect: QuickConnectManager::new(
                QuickConnectRepository::new(Arc::clone(&database)),
                Arc::clone(&quick_connect_capability),
            ),
            quick_connect_capability,
            playstate: PlaystateService::new(Arc::clone(&database)),
            playlists: PlaylistService::new(Arc::clone(&database)),
            collections: CollectionService::new(Arc::clone(&database)),
            user_data: UserDataService::new(Arc::clone(&database)),
            artists: ArtistService::new(Arc::clone(&database)),
            genres: GenreService::with_item_by_name_service(
                Arc::clone(&database),
                item_by_name.clone(),
            ),
            game_genres: GameGenreService::with_item_by_name_service(
                Arc::clone(&database),
                item_by_name.clone(),
            ),
            studios: StudioService::with_item_by_name_service(
                Arc::clone(&database),
                item_by_name.clone(),
            ),
            music_genres: MusicGenreService::with_item_by_name_service(
                Arc::clone(&database),
                item_by_name.clone(),
            ),
            persons: PersonService::with_item_by_name_service(
                Arc::clone(&database),
                item_by_name.clone(),
            ),
            dto_images: PersistedDtoImageProjectionService::new(
                BaseItemRepository::new(Arc::clone(&database)),
                BaseItemImageRepository::new(Arc::clone(&database)),
                Arc::clone(&item_images),
            ),
            item_images,
            metadata_refresh,
            emby_metadata_refresh_sender,
            base_items,
            chapters: ChapterRepository::new(Arc::clone(&database)),
            item_values,
            people,
            image_processor: ImageProcessor::with_concurrency::<4>(
                PathBuf::from("cache").join("images"),
            ),
            item_lookup: ItemLookupService::new(Arc::clone(&database)),
            item_update: ItemUpdateService::new(Arc::clone(&database)),
            metadata_editor: MetadataEditorService::new(Arc::clone(&database)),
            localization: LocalizationService,
            server_configuration: ServerConfigurationRepository::new(Arc::clone(&database)),
            user_library,
            search,
            library_controller: LibraryControllerService::new(Arc::clone(&database)),
            media_attachments: MediaAttachmentService::new(Arc::clone(&database)),
            media_segments: MediaSegmentManagerService::new(Arc::clone(&database)),
            media_streams: MediaStreamService::new(Arc::clone(&database)),
            subtitles: SubtitleManager::default(),
            videos: VideoService::new(Arc::clone(&database)),
            years: YearService::with_item_by_name_service(Arc::clone(&database), item_by_name),
            tuner_hosts: TunerHostManager::new(Arc::clone(&database)),
            live_tv_guide: None,
            virtual_folders: Arc::new(VirtualFolderService::new(Arc::clone(&database))),
            user_views: UserViewManagerService::new(Arc::clone(&database)),
            dashboard: DashboardService::default(),
            environment: EnvironmentService::new(),
            plugins: PluginRegistry::default(),
            packages: PackageService::default(),
            scheduled_tasks: scheduled_tasks.shared_handle(),
            library_scan,
            system_logs: SystemLogService::default(),
            system_storage: SystemStorageService::new(),
            trickplay,
            client_event_logger: ClientEventLogger::new("logs"),
            named_configurations: if matches!(database.as_ref(), DatabaseConnection::Disconnected) {
                None
            } else {
                Some(NamedConfigurationRepository::new(Arc::clone(&database)))
            },
            program_data_directory: PathBuf::from("programdata"),
            web_directory: PathBuf::from("web"),
            image_cache_directory: PathBuf::from("cache").join("images"),
            cache_directory: PathBuf::from("cache"),
            internal_metadata_directory: PathBuf::from("metadata"),
            network_manager: Arc::new(NetworkManager::new(
                NetworkConfiguration::default(),
                Vec::new(),
            )),
            remote_stream_client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            transcode_directory: Arc::from(
                std::env::temp_dir()
                    .join("jellyfin-rust")
                    .join("transcodes"),
            ),
            ffmpeg_path: Arc::new(PathBuf::from("ffmpeg")),
            encoder_capabilities: EncoderCapabilities::default(),
            live_streams: LiveStreamRegistry::new(),
            transcode_jobs: Arc::new(TranscodeJobRegistry::new()),
            authentication: DefaultAuthenticationProvider::new(),
            session_manager: SessionManager::new(session_store),
            branding: Arc::new(tokio::sync::RwLock::new(BrandingOptions::default())),
            system_info: PublicSystemInfo {
                local_address: Some(local_address),
                server_name: None,
                version: Some(JELLYFIN_API_VERSION.to_owned()),
                product_name: Some("Jellyfin Server".to_owned()),
                id: Some(Uuid::new_v4().simple().to_string()),
                startup_wizard_completed: Some(false),
                ..PublicSystemInfo::default()
            },
            startup: Arc::new(Mutex::new(startup::StartupState::new(server_name))),
            startup_repository: None,
            database,
            tmdb_api_key: Arc::new(tokio::sync::RwLock::new(Arc::from(""))),
            omdb_api_key: Arc::new(tokio::sync::RwLock::new(Arc::from("2c9d9507"))),
            system_command: Arc::new(|_| {}),
            metrics_enabled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        state.scheduled_tasks.register_library_refresh_executor(
            Arc::clone(&state.library_scan),
            state.metadata_refresh.clone(),
            Arc::clone(&state.tmdb_api_key),
            Arc::clone(&state.omdb_api_key),
            state.server_configuration.clone(),
        );
        state.scheduled_tasks.add_change_listener(Arc::new(move || {
            let tasks = scheduled_tasks.shared_handle();
            let sockets = Arc::clone(&web_sockets);
            tokio::spawn(async move {
                let infos = tasks.list(None, None).await;
                sockets
                    .send_to_administrators("ScheduledTasksInfo", &infos)
                    .await;
            });
        }));
        state.scheduled_tasks.start_scheduler();
        state.scheduled_tasks.with_maintenance_executors(
            Arc::clone(&state.database),
            ActivityLogRepository::new(Arc::clone(&state.database)),
            UserDataRepository::new(Arc::clone(&state.database)),
            KeyframeDataRepository::new(Arc::clone(&state.database)),
            Arc::clone(&state.trickplay),
            chapter_images,
            state.live_tv_guide.as_ref().map(Arc::clone),
        );
        let transcode_jobs = Arc::clone(&state.transcode_jobs);
        let transcode_directory = Arc::clone(&state.transcode_directory);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                for job_id in transcode_jobs
                    .stop_stale_jobs(tokio::time::Duration::from_secs(15 * 60))
                    .await
                {
                    hls_segment::cleanup_transcode_job(&transcode_directory, &job_id).await;
                }
            }
        });
        state
    }

    /// Selects the user managed by the startup wizard.
    ///
    /// # Panics
    ///
    /// Panics if the startup state was cloned before construction finished.
    #[must_use]
    pub fn with_startup_user(mut self, user_id: Uuid) -> Self {
        Arc::get_mut(&mut self.startup)
            .expect("startup state is uniquely owned during construction")
            .get_mut()
            .user_id = Some(user_id);
        self
    }

    /// Replaces the activity-log retention window used by maintenance tasks.
    #[must_use]
    pub fn with_activity_log_retention_days(self, days: i32) -> Self {
        self.scheduled_tasks.set_activity_log_retention_days(days);
        self
    }

    /// Replaces the scheduled-task log retention window.
    #[must_use]
    pub fn with_log_file_retention_days(self, days: i32) -> Self {
        self.scheduled_tasks.set_log_file_retention_days(days);
        self
    }

    /// Applies the persisted library scan fan-out limit during construction.
    /// A zero value retains Jellyfin's automatic CPU-based setting.
    #[must_use]
    pub fn with_library_scan_fanout_concurrency(self, concurrency: usize) -> Self {
        self.library_scan.set_fanout_concurrency(concurrency);
        self
    }

    /// Uses `PostgreSQL` as the source of truth for server startup configuration.
    ///
    /// The repository singleton must be loaded successfully before attaching it.
    /// [`Self::new`] intentionally retains its in-memory behavior for isolated
    /// route tests and disconnected application states.
    #[must_use]
    pub fn with_persistent_startup(mut self, repository: ServerConfigurationRepository) -> Self {
        self.startup_repository = Some(repository);
        self
    }

    /// Replaces the in-memory TMDB API key used by metadata providers.
    ///
    /// # Panics
    ///
    /// Panics if called while the API key is being read or written.
    #[must_use]
    pub fn with_tmdb_api_key(self, api_key: impl Into<String>) -> Self {
        *self
            .tmdb_api_key
            .try_write()
            .expect("TMDB API key is not being read during state construction") =
            Arc::from(api_key.into());
        self
    }

    /// Replaces the TMDB base URL used by interactive item-lookup routes.
    ///
    /// A compatible proxy may be supplied by deployments; route integration
    /// tests use this to avoid contacting the public provider.
    #[must_use]
    pub fn with_item_lookup_tmdb_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.item_lookup = self.item_lookup.with_tmdb_base_url(base_url);
        self
    }

    /// Replaces the `OMDb` API key used by metadata providers.
    ///
    /// # Panics
    ///
    /// Panics if called while the API key is being read or written.
    #[must_use]
    pub fn with_omdb_api_key(self, api_key: impl Into<String>) -> Self {
        *self
            .omdb_api_key
            .try_write()
            .expect("OMDb API key is not being read during state construction") =
            Arc::from(api_key.into());
        self
    }

    /// Replaces the language and country used by online metadata providers.
    #[must_use]
    pub fn with_metadata_locale(
        self,
        language: impl Into<String>,
        country: impl Into<String>,
    ) -> Self {
        self.metadata_refresh
            .set_preferred_locale(language, country);
        self
    }

    /// Replaces the Quick Connect availability used by authentication.
    #[must_use]
    pub fn with_quick_connect_available(self, available: bool) -> Self {
        self.quick_connect_capability.set_enabled(available);
        self
    }

    /// Enables or disables the Prometheus metrics endpoint at runtime.
    #[must_use]
    pub fn with_metrics_enabled(self, enabled: bool) -> Self {
        self.metrics_enabled
            .store(enabled, std::sync::atomic::Ordering::Release);
        self
    }

    /// Replaces the remote subtitle providers exposed by the subtitle API.
    #[must_use]
    pub fn with_subtitle_providers(mut self, providers: Vec<Arc<dyn SubtitleProvider>>) -> Self {
        self.subtitles = SubtitleManager::new(providers);
        self
    }

    /// Replaces the remote lyric providers exposed by the lyrics API.
    #[must_use]
    pub fn with_lyric_providers(
        mut self,
        providers: Vec<Arc<dyn jellyfin_controller::LyricProvider>>,
    ) -> Self {
        self.user_library = self.user_library.with_lyric_providers(providers);
        self
    }

    /// Replaces the registered media-segment provider names used by API filtering.
    #[must_use]
    pub fn with_media_segment_provider_names(mut self, provider_names: Vec<String>) -> Self {
        self.media_segments = self.media_segments.with_provider_names(provider_names);
        self
    }

    /// Adds external search providers to the `/Items` search pipeline.
    #[must_use]
    pub fn with_search_providers(mut self, providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        self.search = self.search.with_providers(providers);
        self
    }

    /// Replaces the handler invoked by `/System/Restart` and `/System/Shutdown`.
    ///
    /// Route tests default to a no-op handler so they never terminate the test
    /// process. The server binary replaces it with the real host command.
    #[must_use]
    pub fn with_system_commands(
        mut self,
        command: impl Fn(SystemCommand) + Send + Sync + 'static,
    ) -> Self {
        self.system_command = Arc::new(command);
        self
    }

    /// Replaces the server instance identifier with a persisted value.
    ///
    /// The server id should be loaded from the database on startup so that
    /// clients see a stable identity across restarts.
    #[must_use]
    pub fn with_server_id(mut self, server_id: String) -> Self {
        self.system_info.id = Some(server_id);
        self
    }

    /// Replaces the branding configuration used by the public branding API.
    #[must_use]
    pub fn with_branding_options(mut self, branding: BrandingOptions) -> Self {
        self.branding = Arc::new(tokio::sync::RwLock::new(branding));
        self.named_configurations = None;
        self
    }

    /// Replaces the plugin dashboard pages exposed by the web configuration API.
    #[must_use]
    pub fn with_dashboard_pages(mut self, pages: Vec<DashboardPage>) -> Self {
        self.dashboard = DashboardService::new(pages);
        self
    }

    /// Replaces the installed plugin metadata exposed by the plugin API.
    #[must_use]
    pub fn with_plugins(mut self, plugins: Vec<jellyfin_model::PluginInfo>) -> Self {
        self.plugins = PluginRegistry::new(plugins);
        self
    }

    /// Replaces package manifests and repositories exposed by the package API.
    #[must_use]
    pub fn with_packages(mut self, packages: Vec<jellyfin_model::PackageInfo>) -> Self {
        self.packages = PackageService::new(packages);
        self
    }

    /// Replaces the installed plugins while retaining runtime installation
    /// details used by plugin file endpoints.
    #[must_use]
    pub fn with_installed_plugins(mut self, plugins: Vec<InstalledPlugin>) -> Self {
        self.plugins = PluginRegistry::from_installed(plugins);
        self
    }

    /// Replaces the top-level directory exposed by the server log endpoint.
    #[must_use]
    pub fn with_log_directory(mut self, log_directory: impl Into<std::path::PathBuf>) -> Self {
        let log_directory = log_directory.into();
        self.system_logs = SystemLogService::new(log_directory.as_path());
        self.client_event_logger = ClientEventLogger::new(log_directory);
        self.scheduled_tasks
            .set_log_directory(self.system_logs.directory());
        self.scheduled_tasks.set_activity_log_retention_days(30);
        self
    }

    /// Replaces the storage directories reported by `/System/Info/Storage`.
    #[must_use]
    pub fn with_storage_paths(
        mut self,
        program_data_directory: impl Into<PathBuf>,
        web_directory: impl Into<PathBuf>,
        image_cache_directory: impl Into<PathBuf>,
        cache_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) -> Self {
        self.program_data_directory = program_data_directory.into();
        self.web_directory = web_directory.into();
        self.image_cache_directory = image_cache_directory.into();
        self.internal_metadata_directory = internal_metadata_directory.into();
        self.user_library
            .set_internal_metadata_directory(self.internal_metadata_directory.as_path());
        self.library_scan
            .set_image_cache_directory(self.image_cache_directory.as_path());
        self.library_scan.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.genres.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.game_genres.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.artists.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.studios.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.music_genres.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.persons.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.years.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.metadata_refresh.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.scheduled_tasks.set_item_by_name_directories(
            self.program_data_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.item_images = Arc::new(ItemImageService::with_storage_directories(
            Arc::clone(&self.database),
            self.image_cache_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        ));
        self.metadata_refresh
            .set_images(Some(Arc::clone(&self.item_images)));
        self.dto_images = PersistedDtoImageProjectionService::new(
            BaseItemRepository::new(Arc::clone(&self.database)),
            BaseItemImageRepository::new(Arc::clone(&self.database)),
            Arc::clone(&self.item_images),
        );
        self.image_processor =
            ImageProcessor::with_concurrency::<4>(self.image_cache_directory.as_path());
        self.trickplay
            .set_storage_directory(self.program_data_directory.join("trickplay"));
        self.cache_directory = cache_directory.into();
        self.scheduled_tasks
            .set_cache_directory(self.cache_directory.as_path());
        self.scheduled_tasks
            .set_chapter_images_directory(self.program_data_directory.join("chapter-images"));
        self.scheduled_tasks
            .set_task_state_path(self.program_data_directory.join("scheduled-tasks.json"));
        self
    }

    /// Replaces the network classifier used by request endpoint APIs.
    #[must_use]
    pub fn with_network_manager(mut self, network_manager: NetworkManager) -> Self {
        self.network_manager = Arc::new(network_manager);
        self
    }

    /// Replaces the Schedules Direct guide refresh service used by Live TV.
    #[must_use]
    pub fn with_guide_refresh_service(mut self, service: GuideRefreshService) -> Self {
        let service = Arc::new(service);
        self.scheduled_tasks
            .register_refresh_guide_executor(Arc::clone(&service));
        self.live_tv_guide = Some(service);
        self
    }

    /// Replaces the directory containing active transcoding output.
    #[must_use]
    pub fn with_transcode_directory(
        mut self,
        transcode_directory: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.transcode_directory = Arc::from(transcode_directory.into());
        self.scheduled_tasks
            .set_transcode_directory(&*self.transcode_directory);
        self.scheduled_tasks
            .set_trickplay_directory(self.program_data_directory.join("trickplay"));
        self
    }

    /// Replaces the `FFmpeg` binary used for transcoding.
    #[must_use]
    pub fn with_ffmpeg_path(mut self, ffmpeg_path: impl Into<std::path::PathBuf>) -> Self {
        let ffmpeg_path = Arc::new(ffmpeg_path.into());
        self.library_scan
            .set_shared_ffmpeg_path(Arc::clone(&ffmpeg_path));
        self.scheduled_tasks
            .set_shared_ffmpeg_path(Arc::clone(&ffmpeg_path));
        self.ffmpeg_path = ffmpeg_path;
        self
    }

    /// Replaces the `FFprobe` executable used for lazy media-source inspection.
    #[must_use]
    pub fn with_ffprobe_path(self, ffprobe_path: impl Into<std::path::PathBuf>) -> Self {
        self.library_scan.set_probe_path(ffprobe_path);
        self
    }

    /// Replaces the trickplay generation settings used by maintenance tasks.
    #[must_use]
    pub fn with_trickplay_options(self, options: jellyfin_model::TrickplayOptions) -> Self {
        self.scheduled_tasks.set_trickplay_options(options);
        self
    }

    /// Replaces the probed encoder capabilities used by transcode decisions.
    #[must_use]
    pub fn with_encoder_capabilities(mut self, capabilities: EncoderCapabilities) -> Self {
        self.encoder_capabilities = capabilities;
        self
    }

    /// Seeds an active transcode job while constructing application state.
    #[must_use]
    pub fn with_transcode_job(
        self,
        job_id: impl Into<String>,
        device_id: &str,
        play_session_id: &str,
        reasons: TranscodeReason,
    ) -> Self {
        let job_id = job_id.into();
        self.transcode_jobs
            .register_for_session(job_id.clone(), device_id, play_session_id);
        self.transcode_jobs.set_transcode_reasons(&job_id, reasons);
        self
    }

    /// Starts the filesystem watcher over all configured library locations.
    ///
    /// The watcher is intentionally best-effort: an empty or unreadable folder
    /// list leaves the server running and only emits a warning.
    pub async fn start_library_watcher(self) -> Self {
        let paths = match self.virtual_folders.list().await {
            Ok(folders) => folders
                .into_iter()
                .filter(|folder| {
                    folder
                        .library_options
                        .as_object()
                        .and_then(|options| {
                            options
                                .get("EnableRealtimeMonitor")
                                .or_else(|| options.get("enableRealtimeMonitor"))
                        })
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
                })
                .flat_map(|folder| folder.locations.into_iter().map(PathBuf::from))
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot list libraries for the watcher");
                Vec::new()
            }
        };
        if let Err(error) = jellyfin_controller::library_watcher::LibraryWatcher::new(
            Arc::clone(&self.library_scan),
            Arc::clone(&self.virtual_folders),
            paths,
        )
        .start()
        {
            tracing::error!(%error, "library watcher failed to start");
        }
        self
    }

    /// Performs best-effort asynchronous cleanup before the HTTP host stops.
    ///
    /// The notification is sent before cancelling transcodes so connected
    /// clients can close their WebSocket gracefully while the server drains
    /// in-flight HTTP requests.
    pub async fn prepare_for_shutdown(&self, command: SystemCommand) {
        self.web_sockets.notify_shutdown(command);
        let stopped = self.transcode_jobs.stop_all().await;
        if !stopped.is_empty() {
            tracing::info!(
                jobs = stopped.len(),
                "stopped transcode jobs during shutdown"
            );
        }
    }

    pub(crate) fn server_id(&self) -> &str {
        self.system_info.id.as_deref().unwrap_or_default()
    }

    /// Loads public server state for protocol-specific API projections.
    pub async fn public_system_info(&self) -> Result<PublicSystemInfo, StatusCode> {
        system::public_system_info(self)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Loads authorized server state for protocol-specific API projections.
    pub async fn system_info(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Result<SystemInfo, Response> {
        system::system_info(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)
    }

    /// Loads branding state for protocol-specific API projections.
    pub async fn branding_options(&self) -> Result<BrandingOptions, StatusCode> {
        branding::branding_options(self)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Loads display preferences for a protocol-specific wire projection.
    pub async fn display_preferences_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        display_preferences_id: &str,
        user_id: Option<&str>,
        item_id: Option<&str>,
        client: Option<String>,
    ) -> Result<DisplayPreferencesDto, Response> {
        let user_id = parse_optional_uuid(user_id)?;
        let item_id = parse_optional_uuid(item_id)?;
        display_preferences::get_for_request(
            self,
            headers,
            uri,
            display_preferences_id,
            user_id,
            item_id,
            client,
        )
        .await
        .map_err(IntoResponse::into_response)
    }

    #[allow(clippy::too_many_arguments)]
    /// Saves display preferences submitted through a protocol-specific DTO.
    pub async fn update_display_preferences_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        display_preferences_id: &str,
        user_id: Option<&str>,
        item_id: Option<&str>,
        client: Option<String>,
        preferences: DisplayPreferencesDto,
    ) -> Result<StatusCode, Response> {
        let user_id = parse_optional_uuid(user_id)?;
        let item_id = parse_optional_uuid(item_id)?;
        display_preferences::update_for_request(
            self,
            headers,
            uri,
            display_preferences_id,
            user_id,
            item_id,
            client,
            preferences,
        )
        .await
        .map_err(IntoResponse::into_response)
    }

    /// Returns user DTOs for protocol adapters that expose Emby's paged user
    /// queries.  Keep the database lookup and image-tag projection batched.
    pub async fn emby_users(
        &self,
        is_hidden: Option<bool>,
        is_disabled: Option<bool>,
    ) -> Result<Vec<UserDto>, Response> {
        let users = self
            .users
            .list_filtered(is_hidden, is_disabled)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        for user in &users {
            authentication::stored_user_policy(user)
                .map_err(ApiError::from)
                .map_err(IntoResponse::into_response)?;
        }
        users_to_dtos_with_server_id(self, users)
            .await
            .map_err(IntoResponse::into_response)
    }

    /// Enforces Emby's administrator-only user query boundary after the
    /// shared protocol middleware has authenticated the request.
    pub async fn require_emby_administrator(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Result<(), Response> {
        authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?
            .require_administrator()
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }

    /// Enforces Emby's administrator boundary before resolving a target user.
    ///
    /// Protocol handlers use this ordering so malformed or unknown targets do
    /// not leak ahead of the generated client's elevated authorization rule.
    pub async fn require_emby_administrator_user(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        user_id: Uuid,
    ) -> Result<(), Response> {
        self.require_emby_administrator(headers, uri).await?;
        self.users
            .get(user_id)
            .await
            .map(|_| ())
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }

    /// Persists Emby's explicit user/item share levels without exposing the
    /// protocol-private table through Jellyfin's root API.
    ///
    /// The checked-in generated contract requires ordinary authenticated-user
    /// access rather than elevation. Authentication deliberately precedes
    /// body and UUID validation, and the repository validates every referenced
    /// row before applying the Cartesian-product mutation atomically.
    #[allow(clippy::result_large_err)]
    pub async fn update_emby_item_access_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        mutation: Option<EmbyItemAccessMutation>,
    ) -> Result<StatusCode, Response> {
        authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        let mutation = mutation.ok_or_else(|| ApiError::InvalidRequest.into_response())?;
        let user_ids = parse_emby_item_access_ids(mutation.user_ids)?;
        let item_ids = parse_emby_item_access_ids(mutation.item_ids)?;
        let access = match mutation.access_level {
            None => None,
            Some(1) => Some(EmbyItemAccessLevel::Read),
            Some(2) => Some(EmbyItemAccessLevel::Write),
            Some(3) => Some(EmbyItemAccessLevel::Manage),
            Some(4) => Some(EmbyItemAccessLevel::ManageDelete),
            Some(_) => return Err(ApiError::InvalidRequest.into_response()),
        };
        EmbyItemAccessRepository::new(Arc::clone(&self.database))
            .replace(&user_ids, &item_ids, access)
            .await
            .map_err(|error| match error {
                EmbyItemAccessStoreError::UserNotFound | EmbyItemAccessStoreError::ItemNotFound => {
                    ApiError::NotFound.into_response()
                }
                EmbyItemAccessStoreError::Database(_) => ApiError::Internal.into_response(),
            })?;
        Ok(StatusCode::OK)
    }

    /// Removes the target user's explicit Emby shares without changing the
    /// Jellyfin library-policy model.
    ///
    /// Emby's generated contract permits the user id to be omitted, in which
    /// case a device session targets itself. Explicit targets retain the
    /// normal self/administrator/API-key boundary. Target authorization and
    /// lookup intentionally precede item-id binding so an ordinary user
    /// cannot probe another user's shared items with a malformed item id.
    #[allow(clippy::result_large_err)]
    pub async fn leave_emby_shared_items_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        mutation: Option<EmbyLeaveSharedItemsMutation>,
    ) -> Result<StatusCode, Response> {
        let identity = authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        let mutation = mutation.ok_or_else(|| ApiError::InvalidRequest.into_response())?;
        let requested_user_id = mutation
            .user_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| ApiError::InvalidRequest.into_response())?;
        let target_user_id = identity
            .target_user_id(requested_user_id)
            .map_err(IntoResponse::into_response)?;
        self.users
            .get(target_user_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;

        let item_ids = parse_emby_item_access_ids(mutation.item_ids)?;
        EmbyItemAccessRepository::new(Arc::clone(&self.database))
            .replace(&[target_user_id], &item_ids, None)
            .await
            .map_err(|error| match error {
                EmbyItemAccessStoreError::UserNotFound | EmbyItemAccessStoreError::ItemNotFound => {
                    ApiError::NotFound.into_response()
                }
                EmbyItemAccessStoreError::Database(_) => ApiError::Internal.into_response(),
            })?;
        Ok(StatusCode::OK)
    }

    /// Resolves the session identity for Emby's legacy party service while
    /// retaining the shared authentication, enabled-user, remote-access, and
    /// parental-schedule checks.
    pub async fn emby_party_session_context_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Result<EmbyPartySessionContext, Response> {
        let identity = authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(match identity {
            authentication::AuthenticatedIdentity::Device(session) => EmbyPartySessionContext {
                session_id: session::jellyfin_session_id(
                    &session.device.app_name,
                    &session.device.device_id,
                ),
                user: Some(EmbyPartyUser {
                    id: session.user.id.simple().to_string(),
                    name: session.user.username,
                }),
                has_now_playing_item: session.device.now_playing_item.is_some(),
            },
            authentication::AuthenticatedIdentity::ApiKey(api_key) => EmbyPartySessionContext {
                // An API key cannot join a party because Emby requires a
                // session with a user. Keep a stable internal key so its
                // user-less Info/Messages/Leave requests remain isolated.
                session_id: format!("api-key:{}", api_key.id),
                user: None,
                has_now_playing_item: false,
            },
        })
    }

    /// Resets Emby's administrator-owned metadata settings and performs a
    /// real full metadata replacement for every requested item.
    ///
    /// Authentication intentionally precedes query validation. The generated
    /// Emby clients send one comma-separated `ItemIds` string, and malformed
    /// inputs from unauthenticated or ordinary users must not leak binder
    /// behavior ahead of the administrator policy.
    #[allow(clippy::result_large_err)]
    pub async fn reset_emby_metadata_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        item_ids: Option<&str>,
    ) -> Result<StatusCode, Response> {
        self.require_emby_administrator(headers, uri).await?;
        let item_ids = parse_emby_metadata_reset_ids(item_ids)?;
        self.enqueue_emby_metadata_reset(item_ids).await
    }

    #[allow(clippy::result_large_err)]
    async fn enqueue_emby_metadata_reset(
        &self,
        item_ids: Vec<Uuid>,
    ) -> Result<StatusCode, Response> {
        let chunk_count = item_ids.len().div_ceil(EMBY_METADATA_REFRESH_CHUNK_SIZE);
        if chunk_count > EMBY_METADATA_REFRESH_QUEUE_CAPACITY {
            return Err((
                StatusCode::BAD_REQUEST,
                "Too many item ids for one metadata reset request",
            )
                .into_response());
        }
        // Reserve every chunk slot before changing persistent state, so queue
        // saturation cannot leave an accepted reset only partly enqueued.
        let refresh_sender = self.emby_metadata_refresh_sender.clone();
        let refresh_permits = refresh_sender.try_reserve_many(chunk_count).map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Metadata refresh queue is full",
            )
                .into_response()
        })?;
        ItemUpdateRepository::new(Arc::clone(&self.database))
            .reset_metadata_settings(&item_ids)
            .await
            .map_err(|error| match error {
                ItemUpdateStoreError::NotFound => ApiError::NotFound.into_response(),
                ItemUpdateStoreError::InvalidValue => ApiError::InvalidRequest.into_response(),
                ItemUpdateStoreError::InvalidMetadata | ItemUpdateStoreError::Database(_) => {
                    ApiError::Internal.into_response()
                }
            })?;

        let tmdb_api_key = Arc::clone(&*self.tmdb_api_key.read().await);
        let omdb_api_key = Arc::clone(&*self.omdb_api_key.read().await);
        for (permit, item_ids) in
            refresh_permits.zip(item_ids.chunks(EMBY_METADATA_REFRESH_CHUNK_SIZE))
        {
            permit.send(QueuedEmbyMetadataRefresh {
                item_ids: item_ids.to_vec(),
                tmdb_api_key: Arc::clone(&tmdb_api_key),
                omdb_api_key: Arc::clone(&omdb_api_key),
            });
        }
        Ok(StatusCode::OK)
    }

    /// Resolves the source of an Emby administrator-only user-data copy before
    /// the adapter binds its body, preserving authorization and source 404
    /// precedence over malformed copy options.
    pub async fn resolve_emby_copy_data_source(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        source_user_id: &str,
    ) -> Result<(Uuid, String), Response> {
        let identity = authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        identity
            .require_administrator()
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        let current_token = identity.access_token().to_owned();
        let source_user_id = Uuid::parse_str(source_user_id)
            .map_err(|_| ApiError::InvalidRequest.into_response())?;
        self.users
            .get(source_user_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        Ok((source_user_id, current_token))
    }

    /// Copies selected Emby user state to all requested users in one
    /// transaction while preserving the shared user invariants.
    pub async fn copy_emby_user_state(
        &self,
        source_user_id: Uuid,
        target_user_ids: &[Uuid],
        options: EmbyUserCopyOptions,
        current_token: &str,
    ) -> Result<(), Response> {
        if target_user_ids.is_empty() {
            return Err(ApiError::InvalidRequest.into_response());
        }
        let has_changes = options.policy || options.configuration || options.user_data;
        let became_disabled = self
            .users
            .copy_to_users(source_user_id, target_user_ids, options.into())
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        self.devices
            .revoke_users_tokens(&became_disabled, Some(current_token))
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        if !has_changes {
            return Ok(());
        }
        let mut ids = target_user_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let users = self
            .users
            .get_many(&ids)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        let dtos = users_to_dtos_with_server_id(self, users)
            .await
            .map_err(IntoResponse::into_response)?;
        for dto in dtos {
            crate::websocket::broadcast_user_updated(
                self,
                &serde_json::to_value(dto).unwrap_or_default(),
            )
            .await;
        }
        Ok(())
    }

    /// Creates an Emby user and applies the selected copy categories in the
    /// same PostgreSQL transaction.
    pub async fn create_emby_user_with_copy(
        &self,
        name: &str,
        source_user_id: Option<Uuid>,
        options: EmbyUserCopyOptions,
    ) -> Result<UserDto, Response> {
        let user = match source_user_id {
            Some(source_user_id) => {
                self.users
                    .create_with_copy(name, source_user_id, options.into())
                    .await
            }
            None => self.users.create(name).await,
        }
        .map_err(ApiError::from)
        .map_err(IntoResponse::into_response)?;
        let dto = user_to_dto_with_server_id(self, user)
            .await
            .map_err(IntoResponse::into_response)?;
        crate::websocket::broadcast_user_updated(
            self,
            &serde_json::to_value(&dto).unwrap_or_default(),
        )
        .await;
        Ok(dto)
    }

    /// Loads one protocol-owned Emby encoding editor object without exposing
    /// it through Jellyfin's `/System/Configuration/{key}` namespace.
    pub async fn emby_encoding_configuration(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, Response> {
        let repository = self
            .named_configurations
            .as_ref()
            .ok_or_else(|| ApiError::Internal.into_response())?;
        match repository.load(&format!("emby-encoding-{key}")).await {
            Ok(configuration) => Ok(Some(configuration.configuration)),
            Err(NamedConfigurationStoreError::NotFound(_)) => Ok(None),
            Err(error) => Err(ApiError::from(error).into_response()),
        }
    }

    /// Atomically persists one protocol-owned Emby encoding editor object.
    pub async fn save_emby_encoding_configuration(
        &self,
        key: &str,
        configuration: serde_json::Value,
    ) -> Result<(), Response> {
        if !configuration.is_object() {
            return Err(ApiError::InvalidRequest.into_response());
        }
        self.named_configurations
            .as_ref()
            .ok_or_else(|| ApiError::Internal.into_response())?
            .save(&format!("emby-encoding-{key}"), configuration)
            .await
            .map(|_| ())
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }

    /// Clears a video's real alternate-source relationships for an Emby
    /// protocol adapter while retaining the shared elevated authorization and
    /// typed-video not-found semantics.
    #[allow(clippy::result_large_err)]
    pub async fn delete_emby_alternate_sources_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        item_id: &str,
    ) -> Result<(), Response> {
        authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?
            .require_administrator()
            .map_err(IntoResponse::into_response)?;
        let item_id =
            Uuid::parse_str(item_id).map_err(|_| ApiError::InvalidRequest.into_response())?;
        self.videos
            .clear_alternate_sources(true, item_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }

    /// Applies Emby's reversible continue-watching suppression flag while
    /// retaining the shared authentication, target-user, visibility, and
    /// user-data projection behavior.
    #[allow(clippy::result_large_err)]
    pub async fn set_emby_hidden_from_resume_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        requested_user_id: &str,
        item_id: &str,
        is_hidden: Option<bool>,
    ) -> Result<UserItemDataDto, Response> {
        let identity = authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        let requested_user_id = Uuid::parse_str(requested_user_id)
            .map_err(|_| ApiError::InvalidRequest.into_response())?;
        // Resolve the target before surfacing malformed item/query values so
        // an ordinary user cannot probe another user's request validation.
        let target_user_id = identity
            .target_user_id(Some(requested_user_id))
            .map_err(IntoResponse::into_response)?;
        let item_id =
            Uuid::parse_str(item_id).map_err(|_| ApiError::InvalidRequest.into_response())?;
        let is_hidden = is_hidden.ok_or_else(|| ApiError::InvalidRequest.into_response())?;
        let update = self
            .user_data
            .set_hidden_from_resume_for_authorized_user(target_user_id, item_id, is_hidden)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        let dto: UserItemDataDto = update.into();
        websocket::broadcast_user_data_changed(self, target_user_id, &dto).await;
        Ok(dto)
    }

    /// Clears one class of remembered Emby stream selections for an
    /// authorized target user using one set-based PostgreSQL update.
    #[allow(clippy::result_large_err)]
    pub async fn clear_emby_track_selections_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        requested_user_id: &str,
        track_type: &str,
    ) -> Result<StatusCode, Response> {
        let identity = authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        let requested_user_id = Uuid::parse_str(requested_user_id)
            .map_err(|_| ApiError::InvalidRequest.into_response())?;
        // Authorize and resolve the target before inspecting TrackType. This
        // preserves 403/404 precedence for another or missing user.
        let target_user_id = identity
            .target_user_id(Some(requested_user_id))
            .map_err(IntoResponse::into_response)?;
        self.users
            .get(target_user_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        let selection = if track_type.eq_ignore_ascii_case("Audio") {
            RememberedTrackSelection::Audio
        } else if track_type.eq_ignore_ascii_case("Subtitle") {
            RememberedTrackSelection::Subtitle
        } else {
            return Err(ApiError::InvalidRequest.into_response());
        };
        self.user_data
            .clear_remembered_track_selection_for_authorized_user(target_user_id, selection)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        Ok(StatusCode::OK)
    }

    /// Persists Emby's per-user item-search report using the generated
    /// contract's sole nullable `WasSearched` field.
    ///
    /// `reported` distinguishes a valid body whose field was omitted/null
    /// from a body extraction failure. Authentication and target-user
    /// authorization deliberately precede body validation so malformed input
    /// cannot reveal details about another user's route.
    #[allow(clippy::result_large_err)]
    pub async fn report_emby_items_searched_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        requested_user_id: &str,
        reported: Option<Option<bool>>,
    ) -> Result<StatusCode, Response> {
        let target_user_id = self
            .resolve_emby_search_state_user(headers, uri, requested_user_id)
            .await?;
        let Some(was_searched) = reported else {
            return Err(ApiError::InvalidRequest.into_response());
        };
        UserSearchStateRepository::new(Arc::clone(&self.database))
            .set(target_user_id, was_searched.unwrap_or(false))
            .await
            .map_err(|_| ApiError::Internal.into_response())?;
        Ok(StatusCode::OK)
    }

    /// Clears Emby's per-user recently-searched state. Repeated clears are
    /// idempotent, but the target user must exist.
    #[allow(clippy::result_large_err)]
    pub async fn clear_emby_recently_searched_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        requested_user_id: &str,
    ) -> Result<StatusCode, Response> {
        let target_user_id = self
            .resolve_emby_search_state_user(headers, uri, requested_user_id)
            .await?;
        UserSearchStateRepository::new(Arc::clone(&self.database))
            .clear(target_user_id)
            .await
            .map_err(|_| ApiError::Internal.into_response())?;
        Ok(StatusCode::OK)
    }

    /// Validates and touches an opened stream for Emby's legacy MediaInfo
    /// operation while preserving its empty-response wire contract.
    ///
    /// The removed official implementation performed an authenticated,
    /// case-insensitive lookup in the global open-stream dictionary. Its
    /// request has no user, item, device, or play-session field, so a stream
    /// known to another authenticated session remains addressable by id.
    #[allow(clippy::result_large_err)]
    pub async fn emby_live_stream_media_info_for_request(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        live_stream_id: Option<&str>,
    ) -> Result<StatusCode, Response> {
        authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        let live_stream_id = live_stream_id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ApiError::InvalidRequest.into_response())?;
        if !self.live_streams.touch_media_info(live_stream_id) {
            return Err(ApiError::NotFound.into_response());
        }
        Ok(StatusCode::OK)
    }

    #[allow(clippy::result_large_err)]
    async fn resolve_emby_search_state_user(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
        requested_user_id: &str,
    ) -> Result<Uuid, Response> {
        let identity = authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        let requested_user_id = Uuid::parse_str(requested_user_id)
            .map_err(|_| ApiError::InvalidRequest.into_response())?;
        let target_user_id = identity
            .target_user_id(Some(requested_user_id))
            .map_err(IntoResponse::into_response)?;
        self.users
            .get(target_user_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        Ok(target_user_id)
    }

    /// Snapshot plugin/package data for the Emby protocol adapter.
    pub fn emby_plugins(&self) -> Vec<jellyfin_model::PluginInfo> {
        self.plugins.plugins()
    }

    pub fn emby_plugin_image(
        &self,
        plugin_id: uuid::Uuid,
    ) -> Option<jellyfin_controller::PluginImage> {
        self.plugins.image_for_plugin(plugin_id)
    }

    pub fn emby_plugin_configuration(
        &self,
        plugin_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, jellyfin_controller::PluginRegistryError> {
        self.plugins.configuration(plugin_id)
    }

    pub fn emby_packages(&self) -> std::sync::Arc<[std::sync::Arc<jellyfin_model::PackageInfo>]> {
        self.packages.list()
    }

    pub fn emby_package(
        &self,
        name: &str,
        assembly_guid: Option<uuid::Uuid>,
    ) -> Result<std::sync::Arc<jellyfin_model::PackageInfo>, jellyfin_controller::PackageError>
    {
        self.packages.get(name, assembly_guid)
    }

    pub async fn require_emby_user(&self, headers: &HeaderMap, uri: &Uri) -> Result<(), Response> {
        authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map(|_| ())
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)
    }
}

fn parse_optional_uuid(value: Option<&str>) -> Result<Option<Uuid>, Response> {
    value
        .filter(|value| !value.is_empty())
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST.into_response())
}

fn parse_emby_metadata_reset_ids(value: Option<&str>) -> Result<Vec<Uuid>, Response> {
    let value = value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::InvalidRequest.into_response())?;
    let mut seen = HashSet::new();
    let mut item_ids = Vec::new();
    for value in value.split(',') {
        let value = value.trim();
        if value.is_empty() {
            return Err(ApiError::InvalidRequest.into_response());
        }
        let item_id =
            Uuid::parse_str(value).map_err(|_| ApiError::InvalidRequest.into_response())?;
        if seen.insert(item_id) {
            item_ids.push(item_id);
        }
    }
    Ok(item_ids)
}

fn parse_emby_item_access_ids(values: Option<Vec<String>>) -> Result<Vec<Uuid>, Response> {
    values
        .unwrap_or_default()
        .into_iter()
        .map(|value| Uuid::parse_str(&value).map_err(|_| ApiError::InvalidRequest.into_response()))
        .collect()
}

#[cfg(test)]
mod emby_metadata_refresh_queue_tests {
    use jellyfin_data::{DatabaseConfig, NewBaseItem};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn saturated_queue_rejects_before_persistent_reset() {
        let database = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let repository = BaseItemRepository::new(database.clone());
        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.data = Some(json!({
            "IsLocked": true,
            "LockedFields": ["Name"],
            "ProviderIds": { "Custom": "opaque" }
        }));
        let original = repository.create(item).await.expect("locked item fixture");
        let state = AppState::new(
            database.clone(),
            "Metadata queue saturation test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_omdb_api_key("");

        let held_permits = state
            .emby_metadata_refresh_sender
            .try_reserve_many(EMBY_METADATA_REFRESH_QUEUE_CAPACITY)
            .expect("empty queue capacity")
            .collect::<Vec<_>>();
        let response = match state.enqueue_emby_metadata_reset(vec![item_id]).await {
            Ok(status) => panic!("saturated queue unexpectedly accepted reset: {status}"),
            Err(response) => response,
        };
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let unchanged = repository
            .get(item_id)
            .await
            .expect("post-rejection item lookup")
            .expect("post-rejection item");
        assert_eq!(unchanged.row_version, original.row_version);
        assert_eq!(unchanged.data, original.data);

        drop(held_permits);
        drop(state);
        repository.delete(item_id).await.expect("fixture cleanup");
        database.close().await.expect("database pool cleanup");
    }
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::needless_pass_by_value)]
pub fn router(state: AppState) -> Router {
    let base = unprefixed_router(state);

    Router::new().nest("/api", base.clone()).merge(base)
}

/// Builds only Jellyfin's unprefixed routes so another protocol crate can
/// reuse the business handlers without inheriting Jellyfin's route prefixes.
#[allow(clippy::too_many_lines)]
#[allow(clippy::needless_pass_by_value)]
pub fn unprefixed_router(state: AppState) -> Router {
    let state = Arc::new(state);
    let base = base_router(Arc::clone(&state));

    base.with_state(state)
}

/// Builds the legacy Emby audio HLS route without adding it to Jellyfin's
/// unprefixed API. Emby's generated clients still call this endpoint, while
/// current Jellyfin exposes audio HLS through master/main playlists.
pub fn emby_legacy_audio_hls_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Audio/{item_id}/live.m3u8",
            get(hls_segment::emby_audio_live_playlist),
        )
        .route(
            "/audio/{item_id}/live.m3u8",
            get(hls_segment::emby_audio_live_playlist),
        )
}

/// Builds Emby's legacy top-level HLS subtitle playlist routes without
/// exposing them from Jellyfin's unprefixed API.
pub fn emby_legacy_subtitle_hls_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Videos/{item_id}/subtitles.m3u8",
            get(subtitles::emby_legacy_subtitle_playlist),
        )
        .route(
            "/Videos/{item_id}/live_subtitles.m3u8",
            get(subtitles::emby_legacy_subtitle_playlist),
        )
        .route(
            "/videos/{item_id}/subtitles.m3u8",
            get(subtitles::emby_legacy_subtitle_playlist),
        )
        .route(
            "/videos/{item_id}/live_subtitles.m3u8",
            get(subtitles::emby_legacy_subtitle_playlist),
        )
}

#[allow(clippy::too_many_lines)]
fn base_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let index_path = state.web_directory.join("index.html");

    let router = openapi::documented_routes()
        .merge(system_routes())
        .merge(sync_play_routes())
        .route("/websocket", get(websocket::connect))
        .route("/socket", get(websocket::connect))
        .route("/Branding/Configuration", get(branding::get_configuration))
        .route("/branding/configuration", get(branding::get_configuration))
        .route("/Branding/Css", get(branding::get_css))
        .route("/Branding/Css.css", get(branding::get_css))
        .route("/branding/css", get(branding::get_css))
        .route("/branding/css.css", get(branding::get_css))
        .route(
            "/Branding/Splashscreen",
            get(branding::get_splashscreen)
                .post(branding::upload_splashscreen)
                .delete(branding::delete_splashscreen),
        )
        .route(
            "/branding/splashscreen",
            get(branding::get_splashscreen)
                .post(branding::upload_splashscreen)
                .delete(branding::delete_splashscreen),
        )
        .route("/Channels", get(channels::list))
        .route("/channels", get(channels::list))
        .route("/Channels/Features", get(channels::all_features))
        .route("/channels/features", get(channels::all_features))
        .route(
            "/Channels/Items/Latest",
            get(channels::latest_channel_items),
        )
        .route(
            "/channels/items/latest",
            get(channels::latest_channel_items),
        )
        .route(
            "/Channels/{channel_id}/Features",
            get(channels::features),
        )
        .route(
            "/channels/{channel_id}/features",
            get(channels::features),
        )
        .route(
            "/Channels/{channel_id}/Items",
            get(channels::channel_items),
        )
        .route(
            "/channels/{channel_id}/items",
            get(channels::channel_items),
        )
        .route(
            "/Artists/{name}/Images/{image_type}/{image_index}",
            get(artists::get_image),
        )
        .route(
            "/Artists/{name}/Images/{image_type}",
            get(artists::get_image_default),
        )
        .route(
            "/artists/{name}/images/{image_type}/{image_index}",
            get(artists::get_image),
        )
        .route(
            "/artists/{name}/images/{image_type}",
            get(artists::get_image_default),
        )
        .route("/Search/Hints", get(search::hints))
        .route("/search/hints", get(search::hints))
        .route("/Backup", get(backup::list))
        .route("/backup", get(backup::list))
        .route("/Backup/Create", post(backup::create))
        .route("/backup/create", post(backup::create))
        .route("/Backup/Manifest", get(backup::manifest))
        .route("/backup/manifest", get(backup::manifest))
        .route("/Backup/Restore", post(backup::restore))
        .route("/backup/restore", post(backup::restore))
        .route("/Items/{item_id}/Images", get(item_images::list))
        .route("/items/{item_id}/images", get(item_images::list))
        .route(
            "/Items/{item_id}/Images/{image_type}",
            get(item_images::get)
                .post(item_images::upload)
                .delete(item_images::delete),
        )
        .route(
            "/items/{item_id}/images/{image_type}",
            get(item_images::get)
                .post(item_images::upload)
                .delete(item_images::delete),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/Delete",
            post(item_images::delete),
        )
        .route(
            "/items/{item_id}/images/{image_type}/delete",
            post(item_images::delete),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}",
            get(item_images::get_by_index)
                .post(item_images::upload_by_index)
                .delete(item_images::delete_by_index),
        )
        .route(
            "/items/{item_id}/images/{image_type}/{image_index}",
            get(item_images::get_by_index)
                .post(item_images::upload_by_index)
                .delete(item_images::delete_by_index),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}/Delete",
            post(item_images::delete_by_index),
        )
        .route(
            "/items/{item_id}/images/{image_type}/{image_index}/delete",
            post(item_images::delete_by_index),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}/Url",
            post(item_images::upload_url),
        )
        .route(
            "/items/{item_id}/images/{image_type}/{image_index}/url",
            post(item_images::upload_url),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}/Index",
            post(item_images::update_index),
        )
        .route(
            "/items/{item_id}/images/{image_type}/{image_index}/index",
            post(item_images::update_index),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}/{tag}/{format}/{max_width}/{max_height}/{percent_played}/{unplayed_count}",
            get(item_images::get_legacy_path),
        )
        .route(
            "/items/{item_id}/images/{image_type}/{image_index}/{tag}/{format}/{max_width}/{max_height}/{percent_played}/{unplayed_count}",
            get(item_images::get_legacy_path),
        )
        .route(
            "/Items/{item_id}/Subtitles/{index}",
            axum::routing::delete(subtitles::delete_subtitle),
        )
        .route(
            "/items/{item_id}/subtitles/{index}",
            axum::routing::delete(subtitles::delete_subtitle),
        )
        .route(
            "/Items/{item_id}/Subtitles/{index}/Delete",
            post(subtitles::delete_subtitle),
        )
        .route(
            "/items/{item_id}/subtitles/{index}/delete",
            post(subtitles::delete_subtitle),
        )
        .route(
            "/Items/{item_id}/{media_source_id}/Subtitles/{index}/Stream.{format}",
            get(subtitles::get_subtitle),
        )
        .route(
            "/items/{item_id}/{media_source_id}/subtitles/{index}/stream.{format}",
            get(subtitles::get_subtitle),
        )
        .route(
            "/Items/{item_id}/{media_source_id}/Subtitles/{index}/{start_position_ticks}/Stream.{format}",
            get(subtitles::get_subtitle_with_ticks),
        )
        .route(
            "/items/{item_id}/{media_source_id}/subtitles/{index}/{start_position_ticks}/stream.{format}",
            get(subtitles::get_subtitle_with_ticks),
        )
        .route("/Items/{item_id}/RemoteImages", get(remote_images::images))
        .route("/items/{item_id}/remoteimages", get(remote_images::images))
        .route("/Images/Remote", get(remote_images::fetch))
        .route("/images/remote", get(remote_images::fetch))
        .route(
            "/Items/{item_id}/RemoteImages/Providers",
            get(remote_images::providers),
        )
        .route(
            "/items/{item_id}/remoteimages/providers",
            get(remote_images::providers),
        )
        .route(
            "/Items/{item_id}/RemoteImages/Download",
            post(remote_images::download),
        )
        .route(
            "/items/{item_id}/remoteimages/download",
            post(remote_images::download),
        )
        .route(
            "/System/Configuration",
            get(configuration::get).post(configuration::update),
        )
        .route(
            "/system/configuration",
            get(configuration::get).post(configuration::update),
        )
        .route("/System/Configuration/Partial", post(configuration::partial))
        .route("/system/configuration/partial", post(configuration::partial))
        .route(
            "/System/Configuration/MetadataOptions/Default",
            get(configuration::default_metadata_options),
        )
        .route(
            "/system/configuration/metadataoptions/default",
            get(configuration::default_metadata_options),
        )
        .route(
            "/System/Configuration/Branding",
            get(configuration::get_branding_named).post(branding::update_configuration),
        )
        .route(
            "/system/configuration/branding",
            get(configuration::get_branding_named).post(branding::update_configuration),
        )
        .route(
            "/System/Configuration/{key}",
            get(configuration::get_named).post(configuration::update_named),
        )
        .route(
            "/system/configuration/{key}",
            get(configuration::get_named).post(configuration::update_named),
        )
        .route("/web/ConfigurationPage", get(dashboard::configuration_page))
        .route("/web/configurationpage", get(dashboard::configuration_page))
        .route(
            "/web/ConfigurationPages",
            get(dashboard::configuration_pages),
        )
        .route(
            "/web/configurationpages",
            get(dashboard::configuration_pages),
        )
        .route("/Playback/BitrateTest", get(media_info::bitrate_test))
        .route("/playback/bitratetest", get(media_info::bitrate_test))
        .route(
            "/Items/{item_id}/PlaybackInfo",
            get(media_info::get_playback_info).post(media_info::post_playback_info),
        )
        .route(
            "/items/{item_id}/playbackinfo",
            get(media_info::get_playback_info).post(media_info::post_playback_info),
        )
        .route("/LiveStreams/Open", post(media_info::open_live_stream))
        .route("/LiveStreams/Close", post(media_info::close_live_stream))
        .route("/livestreams/open", post(media_info::open_live_stream))
        .route("/livestreams/close", post(media_info::close_live_stream))
        .route(
            "/MediaSegments/{item_id}",
            get(media_segments::get_item_segments),
        )
        .route(
            "/mediasegments/{item_id}",
            get(media_segments::get_item_segments),
        )
        .route("/FallbackFont/Fonts", get(subtitles::fallback_fonts))
        .route("/FallbackFont/Fonts/{name}", get(subtitles::fallback_font))
        .route("/fallbackfont/fonts", get(subtitles::fallback_fonts))
        .route(
            "/fallbackfont/fonts/{name}",
            get(subtitles::fallback_font),
        )
        .route(
            "/Audio/{item_id}/hls/{*legacy_path}",
            get(hls_segment::audio),
        )
        .route(
            "/Audio/{item_id}/master.m3u8",
            get(hls_segment::audio_master_playlist).head(hls_segment::audio_master_playlist),
        )
        .route(
            "/Audio/{item_id}/main.m3u8",
            get(hls_segment::audio_main_playlist),
        )
        .route(
            "/Audio/{item_id}/hls1/{playlist_id}/{segment_file}",
            get(hls_segment::audio_hls1_segment),
        )
        .route(
            "/Audio/{item_id}/stream",
            get(audio::stream).head(audio::stream),
        )
        .route(
            "/Audio/{item_id}/stream.{container}",
            get(audio::stream_with_container).head(audio::stream_with_container),
        )
        .route(
            "/Audio/{item_id}/{stream_file_name}",
            get(audio::stream_with_file_name).head(audio::stream_with_file_name),
        )
        .route(
            "/Audio/{item_id}/universal",
            get(audio::universal).head(audio::universal),
        )
        .route(
            "/Audio/{item_id}/universal.{container}",
            get(audio::universal_with_container).head(audio::universal_with_container),
        )
        // ASP.NET routing is case-insensitive and Jellyfin-generated playback
        // URLs use lowercase collection segments. Keep lowercase aliases for
        // clients following those URLs when running on Axum.
        .route(
            "/audio/{item_id}/hls/{*legacy_path}",
            get(hls_segment::audio),
        )
        .route(
            "/audio/{item_id}/master.m3u8",
            get(hls_segment::audio_master_playlist).head(hls_segment::audio_master_playlist),
        )
        .route(
            "/audio/{item_id}/main.m3u8",
            get(hls_segment::audio_main_playlist),
        )
        .route(
            "/audio/{item_id}/hls1/{playlist_id}/{segment_file}",
            get(hls_segment::audio_hls1_segment),
        )
        .route(
            "/audio/{item_id}/stream",
            get(audio::stream).head(audio::stream),
        )
        .route(
            "/audio/{item_id}/stream.{container}",
            get(audio::stream_with_container).head(audio::stream_with_container),
        )
        .route(
            "/audio/{item_id}/{stream_file_name}",
            get(audio::stream_with_file_name).head(audio::stream_with_file_name),
        )
        .route(
            "/audio/{item_id}/universal",
            get(audio::universal).head(audio::universal),
        )
        .route(
            "/audio/{item_id}/universal.{container}",
            get(audio::universal_with_container).head(audio::universal_with_container),
        )
        .route(
            "/Videos/{item_id}/hls/{*legacy_path}",
            get(hls_segment::video),
        )
        .route(
            "/Videos/{item_id}/live.m3u8",
            get(hls_segment::video_live_playlist),
        )
        .route(
            "/Videos/{item_id}/master.m3u8",
            get(hls_segment::video_master_playlist).head(hls_segment::video_master_playlist),
        )
        .route(
            "/Videos/{item_id}/main.m3u8",
            get(hls_segment::video_main_playlist),
        )
        .route(
            "/Videos/{item_id}/hls1/{playlist_id}/{segment_file}",
            get(hls_segment::video_hls1_segment),
        )
        .route(
            "/Videos/ActiveEncodings",
            axum::routing::delete(hls_segment::stop_active_encoding),
        )
        .route(
            "/Videos/ActiveEncodings/Delete",
            post(hls_segment::stop_active_encoding),
        )
        .route(
            "/videos/ActiveEncodings",
            axum::routing::delete(hls_segment::stop_active_encoding),
        )
        .route(
            "/videos/activeencodings",
            axum::routing::delete(hls_segment::stop_active_encoding),
        )
        .route(
            "/videos/activeencodings/delete",
            post(hls_segment::stop_active_encoding),
        )
        .route(
            "/Videos/{item_id}/stream",
            get(videos::stream).head(videos::stream),
        )
        .route(
            "/Videos/{item_id}/stream.{container}",
            get(videos::stream_with_container).head(videos::stream_with_container),
        )
        .route(
            "/Videos/{item_id}/{stream_file_name}",
            get(videos::stream_with_file_name).head(videos::stream_with_file_name),
        )
        .route(
            "/videos/{item_id}/hls/{*legacy_path}",
            get(hls_segment::video),
        )
        .route(
            "/videos/{item_id}/live.m3u8",
            get(hls_segment::video_live_playlist),
        )
        .route(
            "/videos/{item_id}/master.m3u8",
            get(hls_segment::video_master_playlist).head(hls_segment::video_master_playlist),
        )
        .route(
            "/videos/{item_id}/main.m3u8",
            get(hls_segment::video_main_playlist),
        )
        .route(
            "/videos/{item_id}/hls1/{playlist_id}/{segment_file}",
            get(hls_segment::video_hls1_segment),
        )
        .route(
            "/videos/{item_id}/stream",
            get(videos::stream).head(videos::stream),
        )
        .route(
            "/videos/{item_id}/stream.{container}",
            get(videos::stream_with_container).head(videos::stream_with_container),
        )
        .route(
            "/videos/{item_id}/{stream_file_name}",
            get(videos::stream_with_file_name).head(videos::stream_with_file_name),
        )
        .route("/Plugins", get(plugins::list))
        .route("/plugins", get(plugins::list))
        .route(
            "/Plugins/{plugin_id}/{version}/Enable",
            post(plugins::enable),
        )
        .route(
            "/plugins/{plugin_id}/{version}/enable",
            post(plugins::enable),
        )
        .route(
            "/Plugins/{plugin_id}/{version}/Disable",
            post(plugins::disable),
        )
        .route(
            "/plugins/{plugin_id}/{version}/disable",
            post(plugins::disable),
        )
        .route(
            "/Plugins/{plugin_id}/{version}",
            delete(plugins::uninstall_version),
        )
        .route(
            "/plugins/{plugin_id}/{version}",
            delete(plugins::uninstall_version),
        )
        .route("/Plugins/{plugin_id}", delete(plugins::uninstall))
        .route("/plugins/{plugin_id}", delete(plugins::uninstall))
        .route("/Plugins/{plugin_id}/Delete", post(plugins::uninstall))
        .route("/plugins/{plugin_id}/delete", post(plugins::uninstall))
        .route(
            "/Plugins/{plugin_id}/Configuration",
            get(plugins::get_configuration).post(plugins::update_configuration),
        )
        .route(
            "/plugins/{plugin_id}/configuration",
            get(plugins::get_configuration).post(plugins::update_configuration),
        )
        .route("/Plugins/{plugin_id}/Manifest", post(plugins::manifest))
        .route("/plugins/{plugin_id}/manifest", post(plugins::manifest))
        .route("/Plugins/{plugin_id}/{version}/Image", get(plugins::image))
        .route("/plugins/{plugin_id}/{version}/image", get(plugins::image))
        .route("/Plugins/{plugin_id}/Thumb", get(plugins::thumb))
        .route("/plugins/{plugin_id}/thumb", get(plugins::thumb))
        .merge(package_routes())
        .merge(environment_routes())
        .merge(localization_routes())
        .merge(api_key_routes())
        .merge(device_routes())
        .merge(display_preference_routes())
        .merge(user_routes())
        .merge(user_view_routes())
        .merge(startup_routes())
        .merge(authentication_routes())
        .merge(quick_connect_routes())
        .merge(session_routes())
        .merge(playstate_routes())
        .merge(user_data_routes())
        .merge(collection_routes())
        .route(
            "/Users/{user_id}/Items/Root",
            get(user_library::get_root_legacy),
        )
        .route(
            "/users/{user_id}/items/root",
            get(user_library::get_root_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}",
            get(user_library::get_item_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}",
            get(user_library::get_item_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Intros",
            get(user_library::get_intros_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/intros",
            get(user_library::get_intros_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/LocalTrailers",
            get(user_library::get_local_trailers_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/localtrailers",
            get(user_library::get_local_trailers_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/SpecialFeatures",
            get(user_library::get_special_features_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/specialfeatures",
            get(user_library::get_special_features_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Lyrics",
            get(user_library::get_lyrics_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/lyrics",
            get(user_library::get_lyrics_legacy),
        )
        .merge(item_query_routes())
        .merge(library_controller_routes())
        .merge(user_library_routes())
        .merge(video_routes())
        .merge(live_tv_routes())
        .route("/Items/Filters", get(filters::filters_legacy))
        .route("/Items/Filters2", get(filters::filters2))
        .route("/items/filters", get(filters::filters_legacy))
        .route("/items/filters2", get(filters::filters2))
        .route("/Artists", get(artists::list))
        .route("/Artists/AlbumArtists", get(artists::list_album_artists))
        .route("/Artists/{name}", get(artists::get))
        .route("/artists", get(artists::list))
        .route("/artists/albumartists", get(artists::list_album_artists))
        .route("/artists/{name}", get(artists::get))
        .route("/Years", get(years::list))
        .route("/Years/{year}", get(years::get))
        .route("/years", get(years::list))
        .route("/years/{year}", get(years::get))
        .route("/Genres", get(genres::list))
        .route("/Genres/{genre_name}", get(genres::get))
        .route(
            "/Genres/{name}/Images/{image_type}",
            get(genres::get_image),
        )
        .route(
            "/Genres/{name}/Images/{image_type}/{image_index}",
            get(genres::get_image_by_index),
        )
        .route("/genres", get(genres::list))
        .route("/genres/{genre_name}", get(genres::get))
        .route(
            "/genres/{name}/images/{image_type}",
            get(genres::get_image),
        )
        .route(
            "/genres/{name}/images/{image_type}/{image_index}",
            get(genres::get_image_by_index),
        )
        .route("/Studios", get(studios::list))
        .route("/Studios/{name}", get(studios::get))
        .route(
            "/Studios/{name}/Images/{image_type}",
            get(studios::get_image),
        )
        .route(
            "/Studios/{name}/Images/{image_type}/{image_index}",
            get(studios::get_image_by_index),
        )
        .route("/studios", get(studios::list))
        .route("/studios/{name}", get(studios::get))
        .route(
            "/studios/{name}/images/{image_type}",
            get(studios::get_image),
        )
        .route(
            "/studios/{name}/images/{image_type}/{image_index}",
            get(studios::get_image_by_index),
        )
        .route("/Trailers", get(trailers::list))
        .route("/trailers", get(trailers::list))
        .route("/MusicGenres", get(music_genre::list))
        .route("/MusicGenres/{genre_name}", get(music_genre::get))
        .route(
            "/MusicGenres/{name}/Images/{image_type}",
            get(music_genre::get_image),
        )
        .route(
            "/MusicGenres/{name}/Images/{image_type}/{image_index}",
            get(music_genre::get_image_by_index),
        )
        .route("/musicgenres", get(music_genre::list))
        .route("/musicgenres/{genre_name}", get(music_genre::get))
        .route(
            "/musicgenres/{name}/images/{image_type}",
            get(music_genre::get_image),
        )
        .route(
            "/musicgenres/{name}/images/{image_type}/{image_index}",
            get(music_genre::get_image_by_index),
        )
        .route("/Persons", get(persons::list))
        .route("/Persons/{name}", get(persons::get))
        .route(
            "/Persons/{name}/Images/{image_type}",
            get(persons::get_image),
        )
        .route(
            "/Persons/{name}/Images/{image_type}/{image_index}",
            get(persons::get_image_by_index),
        )
        .route("/persons", get(persons::list))
        .route("/persons/{name}", get(persons::get))
        .route(
            "/persons/{name}/images/{image_type}",
            get(persons::get_image),
        )
        .route(
            "/persons/{name}/images/{image_type}/{image_index}",
            get(persons::get_image_by_index),
        )
        .route(
            "/Library/VirtualFolders",
            get(virtual_folders::list)
                .post(virtual_folders::create)
                .delete(virtual_folders::delete),
        )
        .route(
            "/library/virtualfolders",
            get(virtual_folders::list)
                .post(virtual_folders::create)
                .delete(virtual_folders::delete),
        )
        .route(
            "/Library/VirtualFolders/Name",
            post(virtual_folders::rename),
        )
        .route(
            "/library/virtualfolders/name",
            post(virtual_folders::rename),
        )
        .route(
            "/Library/VirtualFolders/Paths",
            post(virtual_folders::add_path).delete(virtual_folders::remove_path),
        )
        .route(
            "/library/virtualfolders/paths",
            post(virtual_folders::add_path).delete(virtual_folders::remove_path),
        )
        .route(
            "/Library/VirtualFolders/Paths/Update",
            post(virtual_folders::update_path),
        )
        .route(
            "/library/virtualfolders/paths/update",
            post(virtual_folders::update_path),
        )
        .route(
            "/Library/VirtualFolders/Query",
            get(virtual_folders::query),
        )
        .route(
            "/library/virtualfolders/query",
            get(virtual_folders::query),
        )
        .route(
            "/Library/VirtualFolders/Delete",
            post(virtual_folders::delete_legacy),
        )
        .route(
            "/library/virtualfolders/delete",
            post(virtual_folders::delete_legacy),
        )
        .route(
            "/Library/VirtualFolders/Paths/Delete",
            post(virtual_folders::remove_path_legacy),
        )
        .route(
            "/library/virtualfolders/paths/delete",
            post(virtual_folders::remove_path_legacy),
        )
        .route(
            "/Library/VirtualFolders/LibraryOptions",
            post(virtual_folders::update_options),
        )
        .route(
            "/library/virtualfolders/libraryoptions",
            post(virtual_folders::update_options),
        )
        .nest_service(
            "/web",
            ServeDir::new(&state.web_directory).fallback(ServeFile::new(&index_path)),
        )
        .fallback(robots::redirect_or_not_found);

    router
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            authorization::require_route_auth,
        ))
        .with_state(state)
}

fn system_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/system/ping", get(ping).post(ping))
        .route("/System/ActivityLog/Entries", get(activity_log::entries))
        .route("/system/activitylog/entries", get(activity_log::entries))
        .route("/System/Logs", get(system::get_logs))
        .route("/system/logs", get(system::get_logs))
        .route("/System/Logs/Query", get(system::query_logs))
        .route("/system/logs/query", get(system::query_logs))
        .route("/System/Logs/Log", get(system::get_log_file))
        .route("/system/logs/log", get(system::get_log_file))
        .route("/System/Logs/{name}", get(system::get_log_file_by_name))
        .route("/system/logs/{name}", get(system::get_log_file_by_name))
        .route("/System/Info", get(system::info))
        .route("/system/info", get(system::info))
        .route("/System/Info/Storage", get(system::storage))
        .route("/system/info/storage", get(system::storage))
        .route("/System/Endpoint", get(system::endpoint_info))
        .route("/system/endpoint", get(system::endpoint_info))
        .route("/System/Ext/ServerDomains", get(system::server_domains))
        .route("/system/ext/serverdomains", get(system::server_domains))
        .route("/System/Restart", post(system::restart))
        .route("/system/restart", post(system::restart))
        .route("/System/Shutdown", post(system::shutdown))
        .route("/system/shutdown", post(system::shutdown))
        .route("/Document", post(client_log::document))
        .route("/ClientLog/Document", post(client_log::document))
        .route("/clientlog/document", post(client_log::document))
        .route("/GetUtcTime", get(time_sync::get_utc_time))
        .route("/getutctime", get(time_sync::get_utc_time))
        .route("/metrics", get(metrics))
        .route("/ScheduledTasks", get(scheduled_tasks::list))
        .route("/scheduledtasks", get(scheduled_tasks::list))
        .route(
            "/ScheduledTasks/Running/{task_id}",
            post(scheduled_tasks::start).delete(scheduled_tasks::stop),
        )
        .route(
            "/scheduledtasks/running/{task_id}",
            post(scheduled_tasks::start).delete(scheduled_tasks::stop),
        )
        .route(
            "/ScheduledTasks/Running/{task_id}/Delete",
            post(scheduled_tasks::stop),
        )
        .route(
            "/scheduledtasks/running/{task_id}/delete",
            post(scheduled_tasks::stop),
        )
        .route(
            "/ScheduledTasks/{task_id}/Triggers",
            post(scheduled_tasks::update_triggers),
        )
        .route(
            "/scheduledtasks/{task_id}/triggers",
            post(scheduled_tasks::update_triggers),
        )
        .route("/ScheduledTasks/{task_id}", get(scheduled_tasks::get))
        .route("/scheduledtasks/{task_id}", get(scheduled_tasks::get))
}

async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    use std::sync::atomic::Ordering;
    if !state.metrics_enabled.load(Ordering::Acquire) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let body = "# HELP jellyfin_up Jellyfin Rust server availability\n# TYPE jellyfin_up gauge\njellyfin_up 1\n";
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}

fn sync_play_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/SyncPlay/New", post(sync_play::create_group))
        .route("/syncplay/new", post(sync_play::create_group))
        .route("/SyncPlay/Join", post(sync_play::join_group))
        .route("/syncplay/join", post(sync_play::join_group))
        .route("/SyncPlay/Leave", post(sync_play::leave_group))
        .route("/syncplay/leave", post(sync_play::leave_group))
        .route("/SyncPlay/List", get(sync_play::list_groups))
        .route("/syncplay/list", get(sync_play::list_groups))
        .route("/SyncPlay/SetNewQueue", post(sync_play::set_new_queue))
        .route("/syncplay/setnewqueue", post(sync_play::set_new_queue))
        .route(
            "/SyncPlay/SetPlaylistItem",
            post(sync_play::set_playlist_item),
        )
        .route(
            "/syncplay/setplaylistitem",
            post(sync_play::set_playlist_item),
        )
        .route(
            "/SyncPlay/RemoveFromPlaylist",
            post(sync_play::remove_from_playlist),
        )
        .route(
            "/syncplay/removefromplaylist",
            post(sync_play::remove_from_playlist),
        )
        .route(
            "/SyncPlay/MovePlaylistItem",
            post(sync_play::move_playlist_item),
        )
        .route(
            "/syncplay/moveplaylistitem",
            post(sync_play::move_playlist_item),
        )
        .route("/SyncPlay/Queue", post(sync_play::queue_items))
        .route("/syncplay/queue", post(sync_play::queue_items))
        .route("/SyncPlay/Unpause", post(sync_play::unpause))
        .route("/syncplay/unpause", post(sync_play::unpause))
        .route("/SyncPlay/Pause", post(sync_play::pause))
        .route("/syncplay/pause", post(sync_play::pause))
        .route("/SyncPlay/Stop", post(sync_play::stop))
        .route("/syncplay/stop", post(sync_play::stop))
        .route("/SyncPlay/Seek", post(sync_play::seek))
        .route("/syncplay/seek", post(sync_play::seek))
        .route("/SyncPlay/Buffering", post(sync_play::buffering))
        .route("/syncplay/buffering", post(sync_play::buffering))
        .route("/SyncPlay/Ready", post(sync_play::ready))
        .route("/syncplay/ready", post(sync_play::ready))
        .route("/SyncPlay/SetIgnoreWait", post(sync_play::set_ignore_wait))
        .route("/syncplay/setignorewait", post(sync_play::set_ignore_wait))
        .route("/SyncPlay/NextItem", post(sync_play::next_item))
        .route("/syncplay/nextitem", post(sync_play::next_item))
        .route("/SyncPlay/PreviousItem", post(sync_play::previous_item))
        .route("/syncplay/previousitem", post(sync_play::previous_item))
        .route("/SyncPlay/SetRepeatMode", post(sync_play::set_repeat_mode))
        .route("/syncplay/setrepeatmode", post(sync_play::set_repeat_mode))
        .route(
            "/SyncPlay/SetShuffleMode",
            post(sync_play::set_shuffle_mode),
        )
        .route(
            "/syncplay/setshufflemode",
            post(sync_play::set_shuffle_mode),
        )
        .route("/SyncPlay/Ping", post(sync_play::ping))
        .route("/syncplay/ping", post(sync_play::ping))
        .route("/SyncPlay/{id}", get(sync_play::get_group))
        .route("/syncplay/{id}", get(sync_play::get_group))
}

fn environment_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Environment/DirectoryContents",
            get(environment::directory_contents),
        )
        .route(
            "/Environment/ValidatePath",
            post(environment::validate_path),
        )
        .route("/Environment/Drives", get(environment::drives))
        .route("/Environment/ParentPath", get(environment::parent_path))
        .route(
            "/Environment/DefaultDirectoryBrowser",
            get(environment::default_directory_browser),
        )
        .route(
            "/environment/directorycontents",
            get(environment::directory_contents),
        )
        .route(
            "/environment/validatepath",
            post(environment::validate_path),
        )
        .route("/environment/drives", get(environment::drives))
        .route("/environment/parentpath", get(environment::parent_path))
        .route(
            "/environment/defaultdirectorybrowser",
            get(environment::default_directory_browser),
        )
}

fn localization_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Localization/Cultures", get(localization::cultures))
        .route("/Localization/cultures", get(localization::cultures))
        .route("/localization/cultures", get(localization::cultures))
        .route("/Localization/Countries", get(localization::countries))
        .route("/Localization/countries", get(localization::countries))
        .route("/localization/countries", get(localization::countries))
        .route(
            "/Localization/ParentalRatings",
            get(localization::parental_ratings),
        )
        .route(
            "/localization/parentalratings",
            get(localization::parental_ratings),
        )
        .route("/Localization/Options", get(localization::options))
        .route("/localization/options", get(localization::options))
}

fn api_key_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Auth/Keys", get(api_keys::list).post(api_keys::create))
        .route("/auth/keys", get(api_keys::list).post(api_keys::create))
        .route("/Auth/Keys/{key}", axum::routing::delete(api_keys::revoke))
        .route(
            "/Auth/Keys/{key}/Delete",
            post(api_keys::revoke).delete(api_keys::revoke),
        )
        .route("/auth/keys/{key}", axum::routing::delete(api_keys::revoke))
        .route(
            "/auth/keys/{key}/delete",
            post(api_keys::revoke).delete(api_keys::revoke),
        )
}

fn package_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Packages", get(packages::list))
        .route("/packages", get(packages::list))
        .route("/Packages/Installed/{name}", post(packages::install))
        .route("/packages/installed/{name}", post(packages::install))
        .route(
            "/Packages/Installing/{package_id}",
            axum::routing::delete(packages::cancel_installation),
        )
        .route(
            "/packages/installing/{package_id}",
            axum::routing::delete(packages::cancel_installation),
        )
        .route(
            "/Packages/Installing/{package_id}/Delete",
            post(packages::cancel_installation),
        )
        .route(
            "/packages/installing/{package_id}/delete",
            post(packages::cancel_installation),
        )
        .route("/Packages/{name}", get(packages::get))
        .route("/packages/{name}", get(packages::get))
        .route(
            "/Repositories",
            get(packages::repositories).post(packages::set_repositories),
        )
        .route(
            "/repositories",
            get(packages::repositories).post(packages::set_repositories),
        )
}

fn startup_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Startup/Configuration",
            get(startup::get_configuration).post(startup::update_configuration),
        )
        .route(
            "/startup/configuration",
            get(startup::get_configuration).post(startup::update_configuration),
        )
        .route("/Startup/RemoteAccess", post(startup::update_remote_access))
        .route("/startup/remoteaccess", post(startup::update_remote_access))
        .route(
            "/Startup/User",
            get(startup::get_user).post(startup::update_user),
        )
        .route(
            "/startup/user",
            get(startup::get_user).post(startup::update_user),
        )
        .route("/Startup/FirstUser", get(startup::get_user))
        .route("/startup/firstuser", get(startup::get_user))
        .route("/Startup/Complete", post(startup::complete))
        .route("/startup/complete", post(startup::complete))
}

fn authentication_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Users/AuthenticateByName",
            post(authentication::authenticate_by_name),
        )
        .route(
            "/Users/authenticatebyname",
            post(authentication::authenticate_by_name),
        )
        .route(
            "/users/authenticatebyname",
            post(authentication::authenticate_by_name),
        )
        .route(
            "/Users/AuthenticateWithQuickConnect",
            post(authentication::authenticate_with_quick_connect),
        )
        .route(
            "/users/authenticatewithquickconnect",
            post(authentication::authenticate_with_quick_connect),
        )
        .route(
            "/Users/{user_id}/Authenticate",
            post(authentication::authenticate),
        )
        .route(
            "/users/{user_id}/authenticate",
            post(authentication::authenticate),
        )
        .route("/Users/Me", get(authentication::current_user))
        .route("/users/me", get(authentication::current_user))
}

fn quick_connect_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/QuickConnect/Enabled", get(quick_connect::enabled))
        .route("/quickconnect/enabled", get(quick_connect::enabled))
        .route("/QuickConnect/Initiate", post(quick_connect::initiate))
        .route("/quickconnect/initiate", post(quick_connect::initiate))
        .route("/QuickConnect/Connect", get(quick_connect::connect))
        .route("/quickconnect/connect", get(quick_connect::connect))
        .route("/QuickConnect/Authorize", post(quick_connect::authorize))
        .route("/quickconnect/authorize", post(quick_connect::authorize))
}

fn device_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Devices", get(devices::list).delete(devices::delete))
        .route("/devices", get(devices::list).delete(devices::delete))
        .route("/Devices/Delete", post(devices::delete))
        .route("/devices/delete", post(devices::delete))
        .route("/Devices/Info", get(devices::info))
        .route("/devices/info", get(devices::info))
        .route(
            "/Devices/Options",
            get(devices::options).post(devices::update_options),
        )
        .route(
            "/devices/options",
            get(devices::options).post(devices::update_options),
        )
}

fn display_preference_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/DisplayPreferences/{display_preferences_id}",
            get(display_preferences::get).post(display_preferences::update),
        )
        .route(
            "/displaypreferences/{display_preferences_id}",
            get(display_preferences::get).post(display_preferences::update),
        )
}

fn user_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/UserImage",
            get(users::get_user_image)
                .post(users::post_user_image)
                .delete(users::delete_user_image),
        )
        .route(
            "/userimage",
            get(users::get_user_image)
                .post(users::post_user_image)
                .delete(users::delete_user_image),
        )
        .route("/Users", get(users::list).post(users::update))
        .route("/users", get(users::list).post(users::update))
        .route("/Users/Public", get(users::list_public))
        .route("/users/public", get(users::list_public))
        .route("/Users/New", post(users::create))
        .route("/users/new", post(users::create))
        .route("/Users/ForgotPassword", post(users::forgot_password))
        .route("/users/forgotpassword", post(users::forgot_password))
        .route(
            "/Users/ForgotPassword/Pin",
            post(users::forgot_password_pin),
        )
        .route(
            "/users/forgotpassword/pin",
            post(users::forgot_password_pin),
        )
        .route("/Users/Configuration", post(users::update_configuration))
        .route("/users/configuration", post(users::update_configuration))
        .route(
            "/Users/{id}",
            get(users::get)
                .post(users::update_legacy)
                .delete(users::delete),
        )
        .route(
            "/users/{id}",
            get(users::get)
                .post(users::update_legacy)
                .delete(users::delete),
        )
        .route("/Users/{id}/Delete", post(users::delete))
        .route("/users/{id}/delete", post(users::delete))
        .route("/User/{id}", axum::routing::delete(users::delete))
        .route("/Users/Password", post(users::update_password_query))
        .route("/users/password", post(users::update_password_query))
        .route(
            "/Users/{id}/Configuration",
            post(users::update_configuration_legacy),
        )
        .route(
            "/users/{id}/configuration",
            post(users::update_configuration_legacy),
        )
        .route(
            "/Users/{id}/Configuration/Partial",
            post(users::update_configuration_partial),
        )
        .route(
            "/users/{id}/configuration/partial",
            post(users::update_configuration_partial),
        )
        .route(
            "/Users/{id}/Images/{image_type}",
            get(users::get_user_image_legacy)
                .post(users::post_user_image_legacy)
                .delete(users::delete_user_image_legacy),
        )
        .route(
            "/Users/{id}/Images/{image_type}/Delete",
            post(users::delete_user_image_legacy),
        )
        .route(
            "/users/{id}/images/{image_type}",
            get(users::get_user_image_legacy)
                .post(users::post_user_image_legacy)
                .delete(users::delete_user_image_legacy),
        )
        .route(
            "/users/{id}/images/{image_type}/delete",
            post(users::delete_user_image_legacy),
        )
        .route(
            "/Users/{id}/Images/{image_type}/{index}",
            get(users::get_user_image_index_legacy)
                .post(users::post_user_image_index_legacy)
                .delete(users::delete_user_image_index_legacy),
        )
        .route(
            "/Users/{id}/Images/{image_type}/{index}/Delete",
            post(users::delete_user_image_index_legacy),
        )
        .route(
            "/users/{id}/images/{image_type}/{index}",
            get(users::get_user_image_index_legacy)
                .post(users::post_user_image_index_legacy)
                .delete(users::delete_user_image_index_legacy),
        )
        .route(
            "/users/{id}/images/{image_type}/{index}/delete",
            post(users::delete_user_image_index_legacy),
        )
        .route("/Users/{id}/Password", post(users::update_password))
        .route("/users/{id}/password", post(users::update_password))
        .route("/Users/{id}/Policy", post(users::update_policy))
        .route("/users/{id}/policy", post(users::update_policy))
}

fn user_view_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/UserViews", get(user_views::get))
        .route("/userviews", get(user_views::get))
        .route(
            "/UserViews/GroupingOptions",
            get(user_views::grouping_options),
        )
        .route(
            "/userviews/groupingoptions",
            get(user_views::grouping_options),
        )
        .route("/Users/{user_id}/Views", get(user_views::get_legacy))
        .route("/users/{user_id}/views", get(user_views::get_legacy))
        .route(
            "/Users/{user_id}/GroupingOptions",
            get(user_views::grouping_options_legacy),
        )
        .route(
            "/users/{user_id}/groupingoptions",
            get(user_views::grouping_options_legacy),
        )
}

fn session_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sessions", get(session::list))
        .route("/sessions", get(session::list))
        .route("/Sessions/PlayQueue", get(session::play_queue))
        .route("/sessions/playqueue", get(session::play_queue))
        .route(
            "/Sessions/{session_id}/System/{command}",
            post(session::send_system_command),
        )
        .route(
            "/sessions/{session_id}/system/{command}",
            post(session::send_system_command),
        )
        .route(
            "/Sessions/{session_id}/Viewing",
            post(session::display_content),
        )
        .route(
            "/sessions/{session_id}/viewing",
            post(session::display_content),
        )
        .route(
            "/Sessions/{session_id}/Playing",
            post(session::send_play_command),
        )
        .route(
            "/sessions/{session_id}/playing",
            post(session::send_play_command),
        )
        .route(
            "/Sessions/{session_id}/Playing/{command}",
            post(session::send_playstate_command),
        )
        .route(
            "/sessions/{session_id}/playing/{command}",
            post(session::send_playstate_command),
        )
        .route(
            "/Sessions/{session_id}/Command/{command}",
            post(session::send_general_command),
        )
        .route(
            "/sessions/{session_id}/command/{command}",
            post(session::send_general_command),
        )
        .route(
            "/Sessions/{session_id}/Command",
            post(session::send_full_general_command),
        )
        .route(
            "/sessions/{session_id}/command",
            post(session::send_full_general_command),
        )
        .route(
            "/Sessions/{session_id}/Message",
            post(session::send_message_command),
        )
        .route(
            "/sessions/{session_id}/message",
            post(session::send_message_command),
        )
        .route(
            "/Sessions/{session_id}/User/{user_id}",
            post(session::add_user_to_session).delete(session::remove_user_from_session),
        )
        .route(
            "/Sessions/{session_id}/Users/{user_id}",
            post(session::add_user_to_session).delete(session::remove_user_from_session),
        )
        .route(
            "/sessions/{session_id}/user/{user_id}",
            post(session::add_user_to_session).delete(session::remove_user_from_session),
        )
        .route(
            "/sessions/{session_id}/users/{user_id}",
            post(session::add_user_to_session).delete(session::remove_user_from_session),
        )
        .route(
            "/Sessions/{session_id}/Users/{user_id}/Delete",
            post(session::remove_user_from_session),
        )
        .route(
            "/sessions/{session_id}/users/{user_id}/delete",
            post(session::remove_user_from_session),
        )
        .route("/Sessions/Viewing", post(session::report_viewing))
        .route("/sessions/viewing", post(session::report_viewing))
        .route("/Sessions/Capabilities", post(session::post_capabilities))
        .route("/sessions/capabilities", post(session::post_capabilities))
        .route(
            "/Sessions/Capabilities/Full",
            post(session::post_full_capabilities),
        )
        .route(
            "/sessions/capabilities/full",
            post(session::post_full_capabilities),
        )
        .route("/Sessions/Logout", post(session::logout))
        .route("/sessions/logout", post(session::logout))
        .route("/Auth/Providers", get(session::authentication_providers))
        .route("/auth/providers", get(session::authentication_providers))
        .route(
            "/Auth/PasswordResetProviders",
            get(session::password_reset_providers),
        )
        .route(
            "/auth/passwordresetproviders",
            get(session::password_reset_providers),
        )
}

fn playstate_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sessions/Playing", post(playstate::report_playback_start))
        .route(
            "/Sessions/Playing/Progress",
            post(playstate::report_playback_progress),
        )
        .route(
            "/Sessions/Playing/Ping",
            post(playstate::ping_playback_session),
        )
        .route(
            "/Sessions/Playing/Stopped",
            post(playstate::report_playback_stopped),
        )
        // Official Jellyfin is hosted by ASP.NET and treats route casing as
        // insensitive. Several third-party players lowercase these callbacks.
        .route("/sessions/playing", post(playstate::report_playback_start))
        .route(
            "/sessions/playing/progress",
            post(playstate::report_playback_progress),
        )
        .route(
            "/sessions/playing/ping",
            post(playstate::ping_playback_session),
        )
        .route(
            "/sessions/playing/stopped",
            post(playstate::report_playback_stopped),
        )
        .route(
            "/PlayingItems/{item_id}",
            post(playstate::report_playback_start_legacy)
                .delete(playstate::report_playback_stopped_legacy),
        )
        .route(
            "/PlayingItems/{item_id}/Progress",
            post(playstate::report_playback_progress_legacy),
        )
        .route(
            "/Users/{user_id}/PlayingItems/{item_id}",
            post(playstate::report_playback_start_legacy_for_user)
                .delete(playstate::report_playback_stopped_legacy_for_user),
        )
        .route(
            "/Users/{user_id}/PlayingItems/{item_id}/Progress",
            post(playstate::report_playback_progress_legacy_for_user),
        )
        .route(
            "/playingitems/{item_id}",
            post(playstate::report_playback_start_legacy)
                .delete(playstate::report_playback_stopped_legacy),
        )
        .route(
            "/playingitems/{item_id}/progress",
            post(playstate::report_playback_progress_legacy),
        )
        .route(
            "/users/{user_id}/playingitems/{item_id}",
            post(playstate::report_playback_start_legacy_for_user)
                .delete(playstate::report_playback_stopped_legacy_for_user),
        )
        .route(
            "/users/{user_id}/playingitems/{item_id}/progress",
            post(playstate::report_playback_progress_legacy_for_user),
        )
        .route(
            "/UserPlayedItems/{item_id}",
            post(playstate::mark_played_modern).delete(playstate::mark_unplayed_modern),
        )
        .route(
            "/userplayeditems/{item_id}",
            post(playstate::mark_played_modern).delete(playstate::mark_unplayed_modern),
        )
        .route(
            "/Users/{user_id}/PlayedItems/{item_id}",
            post(playstate::mark_played).delete(playstate::mark_unplayed),
        )
        .route(
            "/Users/{user_id}/PlayedItems/{item_id}/Delete",
            post(playstate::mark_unplayed),
        )
        .route(
            "/users/{user_id}/playeditems/{item_id}",
            post(playstate::mark_played).delete(playstate::mark_unplayed),
        )
        .route(
            "/users/{user_id}/playeditems/{item_id}/delete",
            post(playstate::mark_unplayed),
        )
        .route(
            "/Users/{user_id}/PlayingItems/{item_id}/Delete",
            post(playstate::report_playback_stopped_legacy_for_user),
        )
        .route(
            "/users/{user_id}/playingitems/{item_id}/delete",
            post(playstate::report_playback_stopped_legacy_for_user),
        )
}

fn user_data_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/UserItems/{item_id}/UserData",
            get(user_data::get_item_data_modern).post(user_data::update_item_data_modern),
        )
        .route(
            "/useritems/{item_id}/userdata",
            get(user_data::get_item_data_modern).post(user_data::update_item_data_modern),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/UserData",
            get(user_data::get_item_data_legacy).post(user_data::update_item_data_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/userdata",
            get(user_data::get_item_data_legacy).post(user_data::update_item_data_legacy),
        )
        .route(
            "/UserFavoriteItems/{item_id}",
            post(user_data::mark_favorite_modern).delete(user_data::unmark_favorite_modern),
        )
        .route(
            "/userfavoriteitems/{item_id}",
            post(user_data::mark_favorite_modern).delete(user_data::unmark_favorite_modern),
        )
        .route(
            "/Users/{user_id}/FavoriteItems/{item_id}",
            post(user_data::mark_favorite_legacy).delete(user_data::unmark_favorite_legacy),
        )
        .route(
            "/users/{user_id}/favoriteitems/{item_id}",
            post(user_data::mark_favorite_legacy).delete(user_data::unmark_favorite_legacy),
        )
        .route(
            "/Users/{user_id}/FavoriteItems/{item_id}/Delete",
            post(user_data::unmark_favorite_legacy),
        )
        .route(
            "/users/{user_id}/favoriteitems/{item_id}/delete",
            post(user_data::unmark_favorite_legacy),
        )
        .route(
            "/UserItems/{item_id}/Rating",
            post(user_data::set_rating_modern).delete(user_data::delete_rating_modern),
        )
        .route(
            "/useritems/{item_id}/rating",
            post(user_data::set_rating_modern).delete(user_data::delete_rating_modern),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Rating",
            post(user_data::set_rating_legacy).delete(user_data::delete_rating_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/rating",
            post(user_data::set_rating_legacy).delete(user_data::delete_rating_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Rating/Delete",
            post(user_data::delete_rating_legacy),
        )
        .route(
            "/users/{user_id}/items/{item_id}/rating/delete",
            post(user_data::delete_rating_legacy),
        )
}

fn item_query_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items", get(items::get).delete(library::delete_items))
        .route("/items", get(items::get).delete(library::delete_items))
        .route("/Items/Delete", post(library::delete_items))
        .route("/items/delete", post(library::delete_items))
        .route("/Items/Suggestions", get(items::suggestions))
        .route("/items/suggestions", get(items::suggestions))
        .route("/Items/Latest", get(items::latest))
        .route("/items/latest", get(items::latest))
        .route("/UserItems/Resume", get(items::resume))
        .route("/useritems/resume", get(items::resume))
        .route("/Users/{user_id}/Items", get(items::get_legacy))
        .route("/Users/{user_id}/Items/", get(items::get_legacy))
        .route("/Users/{user_id}/Items//", get(items::get_legacy))
        .route("/users/{user_id}/items", get(items::get_legacy))
        .route("/users/{user_id}/items/", get(items::get_legacy))
        .route("/users/{user_id}/items//", get(items::get_legacy))
        .route(
            "/Users/{user_id}/Suggestions",
            get(items::suggestions_legacy),
        )
        .route(
            "/users/{user_id}/suggestions",
            get(items::suggestions_legacy),
        )
        .route("/Users/{user_id}/Items/Latest", get(items::latest_legacy))
        .route("/users/{user_id}/items/latest", get(items::latest_legacy))
        .route("/Users/{user_id}/Items/Resume", get(items::resume_legacy))
        .route("/users/{user_id}/items/resume", get(items::resume_legacy))
}

fn collection_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Collections", post(collections::create))
        .route("/collections", post(collections::create))
        .route(
            "/Collections/{collection_id}/Items",
            post(collections::add_items).delete(collections::remove_items),
        )
        .route(
            "/Collections/{collection_id}/Items/Delete",
            post(collections::remove_items),
        )
        .route(
            "/collections/{collection_id}/items",
            post(collections::add_items).delete(collections::remove_items),
        )
        .route(
            "/collections/{collection_id}/items/delete",
            post(collections::remove_items),
        )
        .route("/Playlists", post(playlists::create))
        .route("/playlists", post(playlists::create))
        .route(
            "/Playlists/{playlist_id}",
            get(playlists::get).post(playlists::update),
        )
        .route(
            "/playlists/{playlist_id}",
            get(playlists::get).post(playlists::update),
        )
        .route("/Playlists/{playlist_id}/Users", get(playlists::get_users))
        .route("/playlists/{playlist_id}/users", get(playlists::get_users))
        .route(
            "/Playlists/{playlist_id}/Users/{user_id}",
            get(playlists::get_user)
                .post(playlists::set_user)
                .delete(playlists::remove_user),
        )
        .route(
            "/playlists/{playlist_id}/users/{user_id}",
            get(playlists::get_user)
                .post(playlists::set_user)
                .delete(playlists::remove_user),
        )
        .route(
            "/Playlists/{playlist_id}/Items",
            get(playlists::get_items)
                .post(playlists::add_items)
                .delete(playlists::remove_items),
        )
        .route(
            "/Playlists/{playlist_id}/Items/Delete",
            post(playlists::remove_items),
        )
        .route(
            "/playlists/{playlist_id}/items",
            get(playlists::get_items)
                .post(playlists::add_items)
                .delete(playlists::remove_items),
        )
        .route(
            "/playlists/{playlist_id}/items/delete",
            post(playlists::remove_items),
        )
        .route(
            "/Playlists/{playlist_id}/AddToPlaylistInfo",
            get(playlists::add_to_playlist_info),
        )
        .route(
            "/playlists/{playlist_id}/addtoplaylistinfo",
            get(playlists::add_to_playlist_info),
        )
        .route(
            "/Playlists/{playlist_id}/Items/{item_id}/Move/{new_index}",
            post(playlists::move_item),
        )
        .route(
            "/playlists/{playlist_id}/items/{item_id}/move/{new_index}",
            post(playlists::move_item),
        )
}

fn library_controller_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Songs/{item_id}/InstantMix", get(library::instant_mix))
        .route("/songs/{item_id}/instantmix", get(library::instant_mix))
        .route("/Albums/{item_id}/InstantMix", get(library::instant_mix))
        .route("/albums/{item_id}/instantmix", get(library::instant_mix))
        .route(
            "/Playlists/{item_id}/InstantMix",
            get(library::instant_mix_playlist),
        )
        .route(
            "/playlists/{item_id}/instantmix",
            get(library::instant_mix_playlist),
        )
        .route("/Artists/{item_id}/InstantMix", get(library::instant_mix))
        .route("/artists/{item_id}/instantmix", get(library::instant_mix))
        .route("/Items/{item_id}/InstantMix", get(library::instant_mix))
        .route("/items/{item_id}/instantmix", get(library::instant_mix))
        .route(
            "/MusicGenres/InstantMix",
            get(library::instant_mix_genre_by_id),
        )
        .route(
            "/musicgenres/instantmix",
            get(library::instant_mix_genre_by_id),
        )
        .route("/Artists/InstantMix", get(library::instant_mix_by_id))
        .route("/artists/instantmix", get(library::instant_mix_by_id))
        .route(
            "/MusicGenres/{name}/InstantMix",
            get(library::instant_mix_genre_by_name),
        )
        .route(
            "/musicgenres/{name}/instantmix",
            get(library::instant_mix_genre_by_name),
        )
        .route("/Items/Counts", get(library::item_counts))
        .route("/items/counts", get(library::item_counts))
        .route("/Items/{item_id}/File", get(library::file))
        .route("/items/{item_id}/file", get(library::file))
        .route("/Items/{item_id}/DeleteInfo", get(library::delete_info))
        .route("/items/{item_id}/deleteinfo", get(library::delete_info))
        .route("/Items/{item_id}/ThemeSongs", get(library::theme_songs))
        .route("/items/{item_id}/themesongs", get(library::theme_songs))
        .route("/Items/{item_id}/ThemeVideos", get(library::theme_videos))
        .route("/items/{item_id}/themevideos", get(library::theme_videos))
        .route("/Items/{item_id}/ThemeMedia", get(library::theme_media))
        .route("/items/{item_id}/thememedia", get(library::theme_media))
        .route("/Items/{item_id}/Ancestors", get(library::ancestors))
        .route("/items/{item_id}/ancestors", get(library::ancestors))
        .route("/Items/{item_id}/Download", get(library::download))
        .route("/items/{item_id}/download", get(library::download))
        .route("/Items/{item_id}/Collections", get(library::collections))
        .route("/items/{item_id}/collections", get(library::collections))
        .route("/Library/Refresh", post(library::refresh))
        .route("/library/refresh", post(library::refresh))
        .route("/Library/PhysicalPaths", get(library::physical_paths))
        .route("/library/physicalpaths", get(library::physical_paths))
        .route("/Library/MediaFolders", get(library::media_folders))
        .route("/library/mediafolders", get(library::media_folders))
        .route(
            "/Library/SelectableMediaFolders",
            get(library::media_folders),
        )
        .route(
            "/library/selectablemediafolders",
            get(library::media_folders),
        )
        .route("/Library/Series/Added", post(library::updated_series))
        .route("/library/series/added", post(library::updated_series))
        .route("/Library/Series/Updated", post(library::updated_series))
        .route("/library/series/updated", post(library::updated_series))
        .route("/Library/Movies/Added", post(library::updated_movies))
        .route("/library/movies/added", post(library::updated_movies))
        .route("/Library/Movies/Updated", post(library::updated_movies))
        .route("/library/movies/updated", post(library::updated_movies))
        .route("/Library/Media/Updated", post(library::updated_media))
        .route("/library/media/updated", post(library::updated_media))
        .route(
            "/Libraries/AvailableOptions",
            get(library::available_options),
        )
        .route(
            "/libraries/availableoptions",
            get(library::available_options),
        )
        .route("/Artists/{item_id}/Similar", get(library::similar))
        .route("/artists/{item_id}/similar", get(library::similar))
        .route("/Items/{item_id}/Similar", get(library::similar))
        .route("/items/{item_id}/similar", get(library::similar))
        .route("/Albums/{item_id}/Similar", get(library::similar))
        .route("/albums/{item_id}/similar", get(library::similar))
        .route("/Shows/{item_id}/Similar", get(library::similar))
        .route("/shows/{item_id}/similar", get(library::similar))
        .route("/Movies/Recommendations", get(movies::recommendations))
        .route("/movies/recommendations", get(movies::recommendations))
        .route("/Movies/{item_id}/Similar", get(library::similar))
        .route("/movies/{item_id}/similar", get(library::similar))
        .route("/Shows/NextUp", get(tv_shows::next_up))
        .route("/shows/nextup", get(tv_shows::next_up))
        .route("/Shows/Upcoming", get(tv_shows::upcoming))
        .route("/shows/upcoming", get(tv_shows::upcoming))
        .route("/Shows/{series_id}/Episodes", get(tv_shows::episodes))
        .route("/shows/{series_id}/episodes", get(tv_shows::episodes))
        .route("/Shows/{series_id}/Seasons", get(tv_shows::seasons))
        .route("/shows/{series_id}/seasons", get(tv_shows::seasons))
        .route("/Trailers/{item_id}/Similar", get(library::similar))
        .route("/trailers/{item_id}/similar", get(library::similar))
}

fn user_library_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/Root", get(user_library::get_root))
        .route("/items/root", get(user_library::get_root))
        .route(
            "/Items/{item_id}",
            get(user_library::get_item)
                .post(item_update::update)
                .delete(library::delete_item),
        )
        .route("/Items/{item_id}/Tags/Add", post(item_update::add_tags))
        .route(
            "/Items/{item_id}/Tags/Delete",
            post(item_update::delete_tags),
        )
        .route("/Items/{item_id}/Delete", post(library::delete_item))
        .route(
            "/items/{item_id}",
            get(user_library::get_item)
                .post(item_update::update)
                .delete(library::delete_item),
        )
        .route("/items/{item_id}/tags/add", post(item_update::add_tags))
        .route(
            "/items/{item_id}/tags/delete",
            post(item_update::delete_tags),
        )
        .route("/items/{item_id}/delete", post(library::delete_item))
        .route(
            "/Items/{item_id}/ContentType",
            post(item_update::update_content_type),
        )
        .route(
            "/items/{item_id}/contenttype",
            post(item_update::update_content_type),
        )
        .route("/Items/{item_id}/Refresh", post(item_refresh::refresh))
        .route("/items/{item_id}/refresh", post(item_refresh::refresh))
        .route(
            "/Items/{item_id}/MetadataEditor",
            get(item_update::metadata_editor),
        )
        .route(
            "/items/{item_id}/metadataeditor",
            get(item_update::metadata_editor),
        )
        .route(
            "/Items/{item_id}/ExternalIdInfos",
            get(item_lookup::external_id_infos),
        )
        .route(
            "/items/{item_id}/externalidinfos",
            get(item_lookup::external_id_infos),
        )
        .route("/Items/{item_id}/MakePublic", post(playlists::make_public))
        .route("/items/{item_id}/makepublic", post(playlists::make_public))
        .route(
            "/Items/{item_id}/MakePrivate",
            post(playlists::make_private),
        )
        .route(
            "/items/{item_id}/makeprivate",
            post(playlists::make_private),
        )
        .route(
            "/Items/RemoteSearch/Movie",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/movie",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/Trailer",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/trailer",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/MusicVideo",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/musicvideo",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/Series",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/series",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/BoxSet",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/boxset",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/MusicArtist",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/musicartist",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/MusicAlbum",
            post(item_lookup::remote_search),
        )
        .route(
            "/items/remotesearch/musicalbum",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/Person",
            post(item_lookup::remote_search_elevated),
        )
        .route(
            "/items/remotesearch/person",
            post(item_lookup::remote_search_elevated),
        )
        .route("/Items/RemoteSearch/Book", post(item_lookup::remote_search))
        .route("/items/remotesearch/book", post(item_lookup::remote_search))
        .route("/Items/RemoteSearch/Image", get(remote_images::fetch))
        .route("/items/remotesearch/image", get(remote_images::fetch))
        .route(
            "/Items/RemoteSearch/Apply/{item_id}",
            post(item_lookup::apply_remote_search),
        )
        .route(
            "/items/remotesearch/apply/{item_id}",
            post(item_lookup::apply_remote_search),
        )
        .route("/Items/{item_id}/Intros", get(user_library::get_intros))
        .route("/items/{item_id}/intros", get(user_library::get_intros))
        .route(
            "/Items/{item_id}/LocalTrailers",
            get(user_library::get_local_trailers),
        )
        .route(
            "/items/{item_id}/localtrailers",
            get(user_library::get_local_trailers),
        )
        .route(
            "/Items/{item_id}/SpecialFeatures",
            get(user_library::get_special_features),
        )
        .route(
            "/items/{item_id}/specialfeatures",
            get(user_library::get_special_features),
        )
        .route(
            "/Audio/{item_id}/RemoteSearch/Lyrics",
            get(user_library::search_remote_lyrics),
        )
        .route(
            "/audio/{item_id}/remotesearch/lyrics",
            get(user_library::search_remote_lyrics),
        )
        .route(
            "/Items/{item_id}/RemoteSearch/Subtitles/{id}",
            get(subtitles::search_remote_subtitles).post(subtitles::download_remote_subtitles),
        )
        .route(
            "/items/{item_id}/remotesearch/subtitles/{id}",
            get(subtitles::search_remote_subtitles).post(subtitles::download_remote_subtitles),
        )
        .route(
            "/Audio/{item_id}/RemoteSearch/Lyrics/{lyric_id}",
            post(user_library::download_remote_lyrics),
        )
        .route(
            "/audio/{item_id}/remotesearch/lyrics/{lyric_id}",
            post(user_library::download_remote_lyrics),
        )
        .route(
            "/Audio/{item_id}/Lyrics",
            get(user_library::get_lyrics)
                .post(user_library::upload_lyrics)
                .delete(user_library::delete_lyrics),
        )
        .route(
            "/audio/{item_id}/lyrics",
            get(user_library::get_lyrics)
                .post(user_library::upload_lyrics)
                .delete(user_library::delete_lyrics),
        )
        .route(
            "/Providers/Lyrics/{lyric_id}",
            get(user_library::get_remote_lyrics),
        )
        .route(
            "/providers/lyrics/{lyric_id}",
            get(user_library::get_remote_lyrics),
        )
        .route(
            "/Providers/Subtitles/Subtitles/{subtitle_id}",
            get(subtitles::get_remote_subtitles),
        )
        .route(
            "/providers/subtitles/subtitles/{subtitle_id}",
            get(subtitles::get_remote_subtitles),
        )
}

fn video_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Videos/MergeVersions", post(videos::merge_versions))
        .route("/videos/mergeversions", post(videos::merge_versions))
        .route(
            "/Videos/{item_id}/AlternateSources",
            axum::routing::delete(videos::delete_alternate_sources),
        )
        .route(
            "/videos/{item_id}/alternatesources",
            axum::routing::delete(videos::delete_alternate_sources),
        )
        .route(
            "/Videos/{item_id}/AdditionalParts",
            get(videos::additional_parts),
        )
        .route(
            "/videos/{item_id}/additionalparts",
            get(videos::additional_parts),
        )
        .route(
            "/Videos/{item_id}/Subtitles/{index}",
            axum::routing::delete(subtitles::delete_subtitle),
        )
        .route(
            "/videos/{item_id}/subtitles/{index}",
            axum::routing::delete(subtitles::delete_subtitle),
        )
        .route(
            "/Videos/{item_id}/Subtitles",
            post(subtitles::upload_subtitle),
        )
        .route(
            "/videos/{item_id}/subtitles",
            post(subtitles::upload_subtitle),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/Stream.{format}",
            get(subtitles::get_subtitle),
        )
        .route(
            "/videos/{item_id}/{media_source_id}/subtitles/{index}/stream.{format}",
            get(subtitles::get_subtitle),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/{start_position_ticks}/Stream.{format}",
            get(subtitles::get_subtitle_with_ticks),
        )
        .route(
            "/videos/{item_id}/{media_source_id}/subtitles/{index}/{start_position_ticks}/stream.{format}",
            get(subtitles::get_subtitle_with_ticks),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/subtitles.m3u8",
            get(subtitles::get_subtitle_playlist),
        )
        .route(
            "/videos/{item_id}/{media_source_id}/subtitles/{index}/subtitles.m3u8",
            get(subtitles::get_subtitle_playlist),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Attachments/{index}",
            get(video_attachments::get),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Attachments/{index}/Stream",
            get(video_attachments::get),
        )
        .route(
            "/videos/{item_id}/{media_source_id}/attachments/{index}",
            get(video_attachments::get),
        )
        .route(
            "/videos/{item_id}/{media_source_id}/attachments/{index}/stream",
            get(video_attachments::get),
        )
        .route(
            "/Videos/{item_id}/Trickplay/{width}/tiles.m3u8",
            get(trickplay::playlist),
        )
        .route(
            "/Videos/{item_id}/Trickplay/{width}/{*tile}",
            get(trickplay::tile),
        )
        // ASP.NET route matching is case-insensitive. Keep the fully lowercase
        // form used by legacy clients alongside the generated SDK route.
        .route(
            "/videos/{item_id}/trickplay/{width}/tiles.m3u8",
            get(trickplay::playlist),
        )
        .route(
            "/videos/{item_id}/trickplay/{width}/{*tile}",
            get(trickplay::tile),
        )
}

fn live_tv_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/LiveTv/Info", get(live_tv::info))
        .route("/LiveTv/Channels", get(live_tv::channels))
        .route("/LiveTv/Channels/{channel_id}", get(live_tv::channel))
        .route("/LiveTv/Recordings", get(live_tv::recordings))
        .route("/LiveTv/Recordings/Series", get(live_tv::recording_series))
        .route("/LiveTv/Recordings/Groups", get(live_tv::recording_groups))
        .route(
            "/LiveTv/Recordings/Folders",
            get(live_tv::recording_folders),
        )
        .route(
            "/LiveTv/Recordings/{recording_id}",
            get(live_tv::recording).delete(live_tv::delete_recording),
        )
        .route(
            "/LiveTv/Tuners/{tuner_id}/Reset",
            post(live_tv::reset_tuner),
        )
        .route(
            "/LiveTv/Timers",
            get(live_tv::timers).post(live_tv::create_timer),
        )
        .route("/LiveTv/Timers/Defaults", get(live_tv::timer_defaults))
        .route(
            "/LiveTv/Timers/{timer_id}",
            get(live_tv::timer)
                .post(live_tv::update_timer)
                .delete(live_tv::cancel_timer),
        )
        .route(
            "/LiveTv/Programs",
            get(live_tv::programs).post(live_tv::programs_post),
        )
        .route(
            "/LiveTv/Programs/Recommended",
            get(live_tv::recommended_programs),
        )
        .route("/LiveTv/Programs/{program_id}", get(live_tv::program))
        .route(
            "/LiveTv/SeriesTimers",
            get(live_tv::series_timers).post(live_tv::create_series_timer),
        )
        .route(
            "/LiveTv/SeriesTimers/{timer_id}",
            get(live_tv::series_timer)
                .post(live_tv::update_series_timer)
                .delete(live_tv::cancel_series_timer),
        )
        .route(
            "/LiveTv/ListingProviders/Default",
            get(live_tv::listing_provider_default),
        )
        .route(
            "/LiveTv/ListingProviders",
            post(live_tv::listing_providers_post).delete(live_tv::delete_listing_provider),
        )
        .route(
            "/LiveTv/ListingProviders/SchedulesDirect/Countries",
            get(live_tv::schedules_direct_countries),
        )
        .route(
            "/LiveTv/ChannelMappingOptions",
            get(live_tv::channel_mapping_options),
        )
        .route(
            "/LiveTv/ChannelMappings",
            post(live_tv::set_channel_mapping),
        )
        .route("/LiveTv/TunerHosts/Types", get(live_tv::tuner_host_types))
        .route("/LiveTv/Tuners/Discover", get(live_tv::discover_tuners))
        .route("/LiveTv/Tuners/Discvover", get(live_tv::discover_tuners))
        .route(
            "/LiveTv/LiveRecordings/{recording_id}/stream",
            get(live_tv::live_recording_stream),
        )
        .route(
            "/LiveTv/LiveStreamFiles/{stream_id}/stream.{container}",
            get(live_tv::live_stream_file),
        )
        .route(
            "/LiveTv/TunerHosts",
            post(live_tv::save_tuner_host).delete(live_tv::delete_tuner_host),
        )
        .route(
            "/LiveTv/ListingProviders/SchedulesDirect/Refresh",
            post(live_tv::refresh_guide),
        )
        .route(
            "/LiveTv/ListingProviders/Lineups",
            get(live_tv::listing_provider_lineups),
        )
        .route("/LiveTv/GuideInfo", get(live_tv::guide_info))
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    match jellyfin_data::healthcheck(&state.database).await {
        Ok(()) => (StatusCode::OK, "Healthy").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Unhealthy").into_response(),
    }
}

async fn public_system_info(
    state: State<Arc<AppState>>,
) -> Result<Json<PublicSystemInfo>, ApiError> {
    system::public_info(state).await
}

async fn ping(state: State<Arc<AppState>>) -> Response {
    system::ping(state).await.into_response()
}

pub(crate) async fn user_to_dto_with_server_id(
    state: &AppState,
    user: user::Model,
) -> Result<UserDto, ApiError> {
    let user_id = user.id;
    let mut tags = user_primary_image_tags(state, &[user_id]).await?;
    Ok(user_to_dto_with_server_id_and_tag(
        state,
        user,
        tags.remove(&user_id),
    ))
}

pub(crate) async fn users_to_dtos_with_server_id(
    state: &AppState,
    users: Vec<user::Model>,
) -> Result<Vec<UserDto>, ApiError> {
    let user_ids = users.iter().map(|user| user.id).collect::<Vec<_>>();
    let mut tags = user_primary_image_tags(state, &user_ids).await?;
    Ok(users
        .into_iter()
        .map(|user| {
            let tag = tags.remove(&user.id);
            user_to_dto_with_server_id_and_tag(state, user, tag)
        })
        .collect())
}

pub(crate) async fn user_primary_image_tags(
    state: &AppState,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, ApiError> {
    Ok(state
        .users
        .profile_images(user_ids)
        .await
        .map_err(|_| ApiError::Internal)?
        .into_iter()
        .map(|image| (image.user_id, user_profile_image_tag(&image)))
        .collect())
}

fn user_to_dto_with_server_id_and_tag(
    state: &AppState,
    user: user::Model,
    primary_image_tag: Option<String>,
) -> UserDto {
    let mut dto = user_to_dto(user);
    dto.server_id = Some(state.server_id().to_owned());
    dto.primary_image_tag = primary_image_tag;
    dto
}

fn user_profile_image_tag(image: &user_profile_image::Model) -> String {
    jellyfin_controller::image_cache_tag(&image.path, image.last_modified)
}

pub(crate) fn user_to_dto(user: user::Model) -> UserDto {
    let user_id = user.id;
    let mut policy: UserPolicy = serde_json::from_value(user.policy).unwrap_or_default();
    policy.is_administrator = user.is_administrator;
    policy.is_hidden = user.is_hidden;
    policy.is_disabled = user.is_disabled;
    policy.authentication_provider_id = Some(user.authentication_provider_id);
    policy.password_reset_provider_id = Some(user.password_reset_provider_id);
    policy.invalid_login_attempt_count = user.invalid_login_attempt_count;
    policy.login_attempts_before_lockout = user.login_attempts_before_lockout;
    for schedule in &mut policy.access_schedules {
        // The Rust persistence layer currently embeds schedules in the policy
        // JSON rather than in a separate entity table. Preserve a submitted
        // identity, but populate legacy rows with their owning user so the
        // official required mobile DTO remains decodable.
        if schedule.user_id.is_nil() {
            schedule.user_id = user_id;
        }
    }
    let mut configuration: UserConfiguration =
        serde_json::from_value(user.preferences).unwrap_or_default();
    configuration.enable_local_password = user.enable_local_password;

    UserDto {
        id: user_id,
        name: Some(user.username),
        // Jellyfin retains these deprecated fields for wire compatibility and
        // always reports true. Password state is no longer exposed here.
        has_password: Some(true),
        has_configured_password: Some(true),
        enable_auto_login: Some(user.enable_auto_login),
        last_login_date: user.last_login_date,
        last_activity_date: user.last_activity_date,
        configuration,
        policy,
        ..UserDto::default()
    }
}

#[derive(Debug)]
pub(crate) enum ApiError {
    ActivityLog(ActivityLogError),
    User(UserError),
    Authentication(AuthenticationError),
    AuthenticationStore(AuthenticationStoreError),
    BaseItem(BaseItemError),
    SessionCommandStore(SessionCommandStoreError),
    Playstate(PlaystateError),
    UserData(UserDataServiceError),
    Artist(ArtistError),
    GameGenre(GameGenreError),
    Genre(GenreError),
    Studio(StudioError),
    MusicGenre(MusicGenreError),
    Person(PersonError),
    UserLibrary(UserLibraryError),
    LibraryController(LibraryControllerError),
    Video(VideoError),
    Year(YearError),
    VirtualFolder(VirtualFolderServiceError),
    UserViewManager(UserViewManagerError),
    Dashboard(DashboardError),
    TunerHost(TunerHostError),
    GuideRefresh(GuideRefreshError),
    ItemLookup(ItemLookupError),
    ItemUpdate(ItemUpdateError),
    ItemImage(ItemImageError),
    ImageProcessing(ImageProcessingError),
    MediaAttachment(MediaAttachmentServiceError),
    MediaSegment(MediaSegmentError),
    MediaStream(MediaStreamServiceError),
    MetadataEditor(MetadataEditorError),
    SystemLog(SystemLogError),
    DisplayPreferenceStore(DisplayPreferenceStoreError),
    ServerConfiguration(ServerConfigurationStoreError),
    NamedConfiguration(NamedConfigurationStoreError),
    Environment(EnvironmentError),
    QuickConnect(QuickConnectError),
    ScheduledTask(ScheduledTaskError),
    LibraryScan(LibraryScanError),
    Package(PackageError),
    Trickplay(TrickplayError),
    Collection(CollectionError),
    Playlist(PlaylistError),
    InvalidRequest,
    UnsupportedMediaType,
    PayloadTooLarge,
    NotFound,
    Unauthorized,
    Forbidden,
    Internal,
    UpstreamUnavailable,
    DeviceNotFound,
    DeviceOptionsNotFound,
    SessionNotFound,
}

impl From<ActivityLogError> for ApiError {
    fn from(error: ActivityLogError) -> Self {
        Self::ActivityLog(error)
    }
}

impl From<UserError> for ApiError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl From<AuthenticationError> for ApiError {
    fn from(error: AuthenticationError) -> Self {
        Self::Authentication(error)
    }
}

impl From<AuthenticationStoreError> for ApiError {
    fn from(error: AuthenticationStoreError) -> Self {
        Self::AuthenticationStore(error)
    }
}

impl From<BaseItemError> for ApiError {
    fn from(error: BaseItemError) -> Self {
        Self::BaseItem(error)
    }
}

impl From<SessionCommandStoreError> for ApiError {
    fn from(error: SessionCommandStoreError) -> Self {
        Self::SessionCommandStore(error)
    }
}

impl From<PlaystateError> for ApiError {
    fn from(error: PlaystateError) -> Self {
        Self::Playstate(error)
    }
}

impl From<UserDataServiceError> for ApiError {
    fn from(error: UserDataServiceError) -> Self {
        Self::UserData(error)
    }
}

impl From<ArtistError> for ApiError {
    fn from(error: ArtistError) -> Self {
        Self::Artist(error)
    }
}

impl From<GenreError> for ApiError {
    fn from(error: GenreError) -> Self {
        Self::Genre(error)
    }
}

impl From<GameGenreError> for ApiError {
    fn from(error: GameGenreError) -> Self {
        Self::GameGenre(error)
    }
}

impl From<StudioError> for ApiError {
    fn from(error: StudioError) -> Self {
        Self::Studio(error)
    }
}

impl From<MusicGenreError> for ApiError {
    fn from(error: MusicGenreError) -> Self {
        Self::MusicGenre(error)
    }
}

impl From<PersonError> for ApiError {
    fn from(error: PersonError) -> Self {
        Self::Person(error)
    }
}

impl From<UserLibraryError> for ApiError {
    fn from(error: UserLibraryError) -> Self {
        Self::UserLibrary(error)
    }
}

impl From<LibraryControllerError> for ApiError {
    fn from(error: LibraryControllerError) -> Self {
        Self::LibraryController(error)
    }
}

impl From<VideoError> for ApiError {
    fn from(error: VideoError) -> Self {
        Self::Video(error)
    }
}

impl From<YearError> for ApiError {
    fn from(error: YearError) -> Self {
        Self::Year(error)
    }
}

impl From<VirtualFolderServiceError> for ApiError {
    fn from(error: VirtualFolderServiceError) -> Self {
        Self::VirtualFolder(error)
    }
}

impl From<UserViewManagerError> for ApiError {
    fn from(error: UserViewManagerError) -> Self {
        Self::UserViewManager(error)
    }
}

impl From<DashboardError> for ApiError {
    fn from(error: DashboardError) -> Self {
        Self::Dashboard(error)
    }
}

impl From<TunerHostError> for ApiError {
    fn from(error: TunerHostError) -> Self {
        Self::TunerHost(error)
    }
}

impl From<GuideRefreshError> for ApiError {
    fn from(error: GuideRefreshError) -> Self {
        Self::GuideRefresh(error)
    }
}

impl From<ItemUpdateError> for ApiError {
    fn from(error: ItemUpdateError) -> Self {
        Self::ItemUpdate(error)
    }
}

impl From<ItemImageError> for ApiError {
    fn from(error: ItemImageError) -> Self {
        Self::ItemImage(error)
    }
}

impl From<ImageProcessingError> for ApiError {
    fn from(error: ImageProcessingError) -> Self {
        Self::ImageProcessing(error)
    }
}

impl From<MediaAttachmentServiceError> for ApiError {
    fn from(error: MediaAttachmentServiceError) -> Self {
        Self::MediaAttachment(error)
    }
}

impl From<MediaSegmentError> for ApiError {
    fn from(error: MediaSegmentError) -> Self {
        Self::MediaSegment(error)
    }
}

impl From<MediaStreamServiceError> for ApiError {
    fn from(error: MediaStreamServiceError) -> Self {
        Self::MediaStream(error)
    }
}

impl From<ItemLookupError> for ApiError {
    fn from(error: ItemLookupError) -> Self {
        Self::ItemLookup(error)
    }
}

impl From<MetadataEditorError> for ApiError {
    fn from(error: MetadataEditorError) -> Self {
        Self::MetadataEditor(error)
    }
}

impl From<SystemLogError> for ApiError {
    fn from(error: SystemLogError) -> Self {
        Self::SystemLog(error)
    }
}

impl From<DisplayPreferenceStoreError> for ApiError {
    fn from(error: DisplayPreferenceStoreError) -> Self {
        Self::DisplayPreferenceStore(error)
    }
}

impl From<ServerConfigurationStoreError> for ApiError {
    fn from(error: ServerConfigurationStoreError) -> Self {
        Self::ServerConfiguration(error)
    }
}

impl From<NamedConfigurationStoreError> for ApiError {
    fn from(error: NamedConfigurationStoreError) -> Self {
        Self::NamedConfiguration(error)
    }
}

impl From<EnvironmentError> for ApiError {
    fn from(error: EnvironmentError) -> Self {
        Self::Environment(error)
    }
}

impl From<QuickConnectError> for ApiError {
    fn from(error: QuickConnectError) -> Self {
        Self::QuickConnect(error)
    }
}

impl From<ScheduledTaskError> for ApiError {
    fn from(error: ScheduledTaskError) -> Self {
        Self::ScheduledTask(error)
    }
}

impl From<LibraryScanError> for ApiError {
    fn from(error: LibraryScanError) -> Self {
        Self::LibraryScan(error)
    }
}

impl From<PackageError> for ApiError {
    fn from(error: PackageError) -> Self {
        Self::Package(error)
    }
}

impl From<TrickplayError> for ApiError {
    fn from(error: TrickplayError) -> Self {
        Self::Trickplay(error)
    }
}

impl From<CollectionError> for ApiError {
    fn from(error: CollectionError) -> Self {
        Self::Collection(error)
    }
}

impl From<PlaylistError> for ApiError {
    fn from(error: PlaylistError) -> Self {
        Self::Playlist(error)
    }
}

impl IntoResponse for ApiError {
    #[allow(
        clippy::too_many_lines,
        reason = "the centralized API error table is clearer as one exhaustive match"
    )]
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidRequest | Self::Playstate(PlaystateError::InvalidDatePlayed) => {
                (StatusCode::BAD_REQUEST, "Invalid request")
            }
            Self::UnsupportedMediaType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "Unsupported media type")
            }
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "Payload too large"),
            Self::NotFound => (StatusCode::NOT_FOUND, "Not found"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            Self::Forbidden
            | Self::Playstate(PlaystateError::Forbidden)
            | Self::UserData(UserDataServiceError::Forbidden) => {
                (StatusCode::FORBIDDEN, "Forbidden")
            }
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
            Self::UpstreamUnavailable => {
                (StatusCode::BAD_GATEWAY, "Upstream media source unavailable")
            }
            Self::DeviceNotFound => (StatusCode::NOT_FOUND, "Device not found"),
            Self::DeviceOptionsNotFound => (StatusCode::NOT_FOUND, "Device options not found"),
            Self::SessionNotFound => (StatusCode::NOT_FOUND, "Session not found"),
            Self::Environment(error) => environment_error_response(&error),
            Self::ActivityLog(
                ActivityLogError::EmptyField(_) | ActivityLogError::FieldTooLong { .. },
            ) => (StatusCode::BAD_REQUEST, "Invalid activity log entry"),
            Self::ActivityLog(ActivityLogError::Database(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Activity log persistence failed",
            ),
            Self::User(error) => user_error_response(&error),
            Self::Authentication(AuthenticationError::InvalidCredentials) => {
                (StatusCode::UNAUTHORIZED, "Invalid username or password")
            }
            Self::Authentication(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stored authentication data is invalid",
            ),
            Self::AuthenticationStore(_error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Authentication persistence failed",
            ),
            Self::BaseItem(BaseItemError::NotFound) => (StatusCode::NOT_FOUND, "Item not found"),
            Self::BaseItem(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Item persistence failed"),
            Self::SessionCommandStore(
                SessionCommandStoreError::EmptyField(_)
                | SessionCommandStoreError::FieldTooLong { .. }
                | SessionCommandStoreError::InvalidPayload,
            ) => (StatusCode::BAD_REQUEST, "Invalid session command"),
            Self::SessionCommandStore(SessionCommandStoreError::Database(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Session command persistence failed",
            ),
            Self::Playstate(
                PlaystateError::UserNotFound
                | PlaystateError::ItemNotFound
                | PlaystateError::User(UserError::NotFound)
                | PlaystateError::BaseItem(BaseItemError::NotFound),
            )
            | Self::UserData(
                UserDataServiceError::UserNotFound
                | UserDataServiceError::ItemNotFound
                | UserDataServiceError::User(UserError::NotFound)
                | UserDataServiceError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "User or item not found"),
            Self::UserData(UserDataServiceError::UserData(
                jellyfin_data::UserDataError::InvalidRating
                | jellyfin_data::UserDataError::NegativePlaybackValue,
            ))
            | Self::Playstate(PlaystateError::UserData(
                jellyfin_data::UserDataError::InvalidRating
                | jellyfin_data::UserDataError::NegativePlaybackValue,
            )) => (StatusCode::BAD_REQUEST, "Invalid user data"),
            Self::UserLibrary(
                UserLibraryError::UserNotFound
                | UserLibraryError::ItemNotFound
                | UserLibraryError::LyricsNotFound
                | UserLibraryError::User(UserError::NotFound)
                | UserLibraryError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "User, item, or lyrics not found"),
            Self::UserLibrary(UserLibraryError::Forbidden)
            | Self::Artist(ArtistError::Forbidden)
            | Self::GameGenre(GameGenreError::Forbidden)
            | Self::Genre(GenreError::Forbidden)
            | Self::Studio(StudioError::Forbidden)
            | Self::MusicGenre(MusicGenreError::Forbidden)
            | Self::Person(PersonError::Forbidden) => (StatusCode::FORBIDDEN, "Forbidden"),
            Self::UserLibrary(UserLibraryError::InvalidPolicy(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stored user policy is invalid",
            ),
            Self::UserLibrary(UserLibraryError::InvalidLyricFile) => {
                (StatusCode::BAD_REQUEST, "Invalid lyric file")
            }
            Self::Genre(
                GenreError::NotFound
                | GenreError::UserNotFound
                | GenreError::User(UserError::NotFound)
                | GenreError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Genre or user not found"),
            Self::GameGenre(
                GameGenreError::NotFound
                | GameGenreError::UserNotFound
                | GameGenreError::User(UserError::NotFound)
                | GameGenreError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Game genre or user not found"),
            Self::Artist(
                ArtistError::NotFound
                | ArtistError::UserNotFound
                | ArtistError::User(UserError::NotFound)
                | ArtistError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Artist or user not found"),
            Self::Studio(
                StudioError::NotFound
                | StudioError::UserNotFound
                | StudioError::User(UserError::NotFound)
                | StudioError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Studio or user not found"),
            Self::MusicGenre(
                MusicGenreError::NotFound
                | MusicGenreError::UserNotFound
                | MusicGenreError::User(UserError::NotFound)
                | MusicGenreError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Music genre or user not found"),
            Self::Playstate(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Playstate persistence failed",
            ),
            Self::UserData(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "User data persistence failed",
            ),
            Self::UserLibrary(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Library persistence failed",
            ),
            Self::LibraryController(error) => library_controller_error_response(&error),
            Self::Video(error) => video_error_response(&error),
            Self::Year(error) => year_error_response(&error),
            Self::Genre(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Genre persistence failed",
            ),
            Self::GameGenre(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Game genre persistence failed",
            ),
            Self::Artist(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Artist persistence failed",
            ),
            Self::Studio(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Studio persistence failed",
            ),
            Self::MusicGenre(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Music genre persistence failed",
            ),
            Self::Person(
                PersonError::NotFound
                | PersonError::UserNotFound
                | PersonError::User(UserError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Person or user not found"),
            Self::Person(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Person persistence failed",
            ),
            Self::VirtualFolder(error) => virtual_folder_error_response(&error),
            Self::UserViewManager(UserViewManagerError::User(UserError::NotFound)) => {
                (StatusCode::NOT_FOUND, "User not found")
            }
            Self::UserViewManager(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "User view persistence failed",
            ),
            Self::Dashboard(error) => dashboard_error_response(&error),
            Self::TunerHost(error) => tuner_host_error_response(&error),
            Self::GuideRefresh(error) => guide_refresh_error_response(&error),
            Self::ItemLookup(error) => item_lookup_error_response(&error),
            Self::ItemUpdate(error) => item_update_error_response(&error),
            Self::ItemImage(ItemImageError::NotFound) => {
                (StatusCode::NOT_FOUND, "Item image not found")
            }
            Self::ItemImage(ItemImageError::UnsupportedImageType) => {
                (StatusCode::BAD_REQUEST, "Unsupported item image type")
            }
            Self::ItemImage(ItemImageError::UnsupportedIndexChange) => (
                StatusCode::BAD_REQUEST,
                "Item image type does not support index changes",
            ),
            Self::ItemImage(
                ItemImageError::InvalidRemoteUrl
                | ItemImageError::RemoteImageTooLarge
                | ItemImageError::RemoteDownload(_)
                | ItemImageError::Io(_)
                | ItemImageError::Store(_),
            ) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Item image persistence failed",
            ),
            Self::ImageProcessing(
                ImageProcessingError::InvalidQuality(_)
                | ImageProcessingError::InvalidPercentPlayed
                | ImageProcessingError::UnsupportedOutputFormat(_)
                | ImageProcessingError::NoSupportedOutputFormat
                | ImageProcessingError::InvalidBackgroundColor(_)
                | ImageProcessingError::UnknownSourceFormat(_),
            ) => (StatusCode::BAD_REQUEST, "Invalid image processing request"),
            Self::ImageProcessing(ImageProcessingError::FileAccess { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                (StatusCode::NOT_FOUND, "Item image file not found")
            }
            Self::ImageProcessing(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Image processing failed")
            }
            Self::MediaAttachment(error) => media_attachment_error_response(&error),
            Self::MediaSegment(error) => {
                let _ = error;
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Media segment persistence failed",
                )
            }
            Self::MediaStream(error) => media_stream_error_response(&error),
            Self::MetadataEditor(error) => metadata_editor_error_response(&error),
            Self::SystemLog(error) => system_log_error_response(&error),
            Self::DisplayPreferenceStore(error) => display_preference_error_response(&error),
            Self::ServerConfiguration(_error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Startup configuration persistence failed",
            ),
            Self::NamedConfiguration(error) => named_configuration_error_response(&error),
            Self::QuickConnect(error) => quick_connect_error_response(&error),
            Self::ScheduledTask(ScheduledTaskError::NotFound) => {
                (StatusCode::NOT_FOUND, "Scheduled task not found")
            }
            Self::ScheduledTask(ScheduledTaskError::ExecutorUnavailable) => (
                StatusCode::NOT_IMPLEMENTED,
                "Scheduled task executor unavailable",
            ),
            Self::LibraryScan(error) => library_scan_error_response(&error),
            Self::Package(PackageError::NotFound) => (StatusCode::NOT_FOUND, "Package not found"),
            Self::Trickplay(error) => trickplay_error_response(&error),
            Self::Collection(error) => collection_error_response(&error),
            Self::Playlist(error) => playlist_error_response(&error),
        };
        (status, Json(serde_json::json!({ "Message": message }))).into_response()
    }
}

fn trickplay_error_response(_error: &TrickplayError) -> (StatusCode, &'static str) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Trickplay persistence failed",
    )
}

fn library_scan_error_response(error: &LibraryScanError) -> (StatusCode, &'static str) {
    match error {
        LibraryScanError::AlreadyScanning => {
            (StatusCode::CONFLICT, "Library scan is already in progress")
        }
        LibraryScanError::BaseItem(BaseItemError::ParentNotFound | BaseItemError::NotFound)
        | LibraryScanError::VirtualFolder(
            jellyfin_data::VirtualFolderError::NotFound
            | jellyfin_data::VirtualFolderError::PathNotFound,
        )
        | LibraryScanError::MediaStream(jellyfin_data::MediaStreamStoreError::BaseItemNotFound {
            ..
        })
        | LibraryScanError::MediaAttachment(
            jellyfin_data::MediaAttachmentStoreError::BaseItemNotFound { .. },
        ) => (StatusCode::NOT_FOUND, "Library scan target not found"),
        LibraryScanError::VirtualFolder(
            jellyfin_data::VirtualFolderError::InvalidName
            | jellyfin_data::VirtualFolderError::DuplicateName
            | jellyfin_data::VirtualFolderError::PathOverlap,
        ) => (
            StatusCode::BAD_REQUEST,
            "Library scan configuration is invalid",
        ),
        LibraryScanError::MediaStream(
            jellyfin_data::MediaStreamStoreError::DuplicateStreamIndex { .. }
            | jellyfin_data::MediaStreamStoreError::InvalidStreamType(_),
        )
        | LibraryScanError::MediaAttachment(
            jellyfin_data::MediaAttachmentStoreError::DuplicateAttachmentIndex { .. },
        ) => (
            StatusCode::BAD_REQUEST,
            "Library scan media metadata is invalid",
        ),
        LibraryScanError::Io(_)
        | LibraryScanError::MediaItemFailures { .. }
        | LibraryScanError::BaseItem(_)
        | LibraryScanError::Chapter(_)
        | LibraryScanError::ItemImage(_)
        | LibraryScanError::ItemValue(_)
        | LibraryScanError::ServerConfiguration(_)
        | LibraryScanError::ItemUpdate(_)
        | LibraryScanError::Person(_)
        | LibraryScanError::MediaStream(jellyfin_data::MediaStreamStoreError::Database(_))
        | LibraryScanError::MediaAttachment(jellyfin_data::MediaAttachmentStoreError::Database(
            _,
        ))
        | LibraryScanError::VirtualFolder(jellyfin_data::VirtualFolderError::Database(_)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Library scan failed")
        }
    }
}

fn collection_error_response(error: &CollectionError) -> (StatusCode, &'static str) {
    use jellyfin_data::{CollectionStoreError, LinkedChildStoreError};

    match error {
        CollectionError::InvalidCollection
        | CollectionError::InvalidName
        | CollectionError::CollectionStore(
            CollectionStoreError::ParentNotFound
            | CollectionStoreError::ChildNotFound { .. }
            | CollectionStoreError::SelfLink
            | CollectionStoreError::TooManyChildren,
        )
        | CollectionError::LinkedChildStore(
            LinkedChildStoreError::ParentNotFound { .. }
            | LinkedChildStoreError::ChildNotFound { .. }
            | LinkedChildStoreError::SelfLink
            | LinkedChildStoreError::SortOrderOverflow,
        ) => (StatusCode::BAD_REQUEST, "Invalid collection request"),
        CollectionError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::BAD_REQUEST, "Invalid collection request")
        }
        CollectionError::CollectionStore(CollectionStoreError::Database(_))
        | CollectionError::LinkedChildStore(
            LinkedChildStoreError::MoveIndexOutOfBounds { .. }
            | LinkedChildStoreError::Database(_)
            | LinkedChildStoreError::CorruptChildType(_),
        )
        | CollectionError::BaseItem(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Collection persistence failed",
        ),
    }
}

fn playlist_error_response(error: &PlaylistError) -> (StatusCode, &'static str) {
    use jellyfin_data::{LinkedChildStoreError, PlaylistStoreError};

    match error {
        PlaylistError::InvalidName => (StatusCode::BAD_REQUEST, "Playlist name is invalid"),
        PlaylistError::NotFound
        | PlaylistError::Store(PlaylistStoreError::NotFound)
        | PlaylistError::Links(LinkedChildStoreError::ParentNotFound { .. }) => {
            (StatusCode::NOT_FOUND, "Playlist not found")
        }
        PlaylistError::Forbidden => (StatusCode::FORBIDDEN, "Playlist access is forbidden"),
        PlaylistError::Store(
            PlaylistStoreError::UserNotFound { .. }
            | PlaylistStoreError::ItemNotFound { .. }
            | PlaylistStoreError::TooManyItems,
        )
        | PlaylistError::Links(
            LinkedChildStoreError::ChildNotFound { .. }
            | LinkedChildStoreError::SelfLink
            | LinkedChildStoreError::SortOrderOverflow,
        ) => (StatusCode::BAD_REQUEST, "Playlist request is invalid"),
        PlaylistError::Store(
            PlaylistStoreError::CorruptShares(_) | PlaylistStoreError::Database(_),
        )
        | PlaylistError::Links(
            LinkedChildStoreError::MoveIndexOutOfBounds { .. }
            | LinkedChildStoreError::CorruptChildType(_)
            | LinkedChildStoreError::Database(_),
        )
        | PlaylistError::Items(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Playlist persistence failed",
        ),
    }
}

fn quick_connect_error_response(error: &QuickConnectError) -> (StatusCode, &'static str) {
    match error {
        QuickConnectError::Disabled => (StatusCode::UNAUTHORIZED, "Quick Connect is disabled"),
        QuickConnectError::NotFound => (StatusCode::NOT_FOUND, "Quick Connect request not found"),
        QuickConnectError::InvalidAuthorization(_)
        | QuickConnectError::AlreadyAuthorized
        | QuickConnectError::Store(
            jellyfin_data::QuickConnectStoreError::EmptyField(_)
            | jellyfin_data::QuickConnectStoreError::FieldTooLong { .. }
            | jellyfin_data::QuickConnectStoreError::InvalidCode
            | jellyfin_data::QuickConnectStoreError::InvalidSecret
            | jellyfin_data::QuickConnectStoreError::InvalidExpiration,
        ) => (StatusCode::BAD_REQUEST, "Invalid Quick Connect request"),
        QuickConnectError::TokenGenerationExhausted
        | QuickConnectError::Store(
            jellyfin_data::QuickConnectStoreError::Conflict
            | jellyfin_data::QuickConnectStoreError::AlreadyAuthorized
            | jellyfin_data::QuickConnectStoreError::NotFound
            | jellyfin_data::QuickConnectStoreError::Device(_)
            | jellyfin_data::QuickConnectStoreError::Database(_),
        ) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Quick Connect persistence failed",
        ),
    }
}

fn named_configuration_error_response(
    error: &NamedConfigurationStoreError,
) -> (StatusCode, &'static str) {
    match error {
        NamedConfigurationStoreError::BlankKey => (
            StatusCode::BAD_REQUEST,
            "Named configuration key must not be blank",
        ),
        NamedConfigurationStoreError::NotFound(_) => {
            (StatusCode::NOT_FOUND, "Named configuration not found")
        }
        NamedConfigurationStoreError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Named configuration persistence failed",
        ),
    }
}

fn environment_error_response(error: &EnvironmentError) -> (StatusCode, &'static str) {
    match error {
        EnvironmentError::NotFound => (StatusCode::NOT_FOUND, "Path not found"),
        EnvironmentError::Io(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "File-system operation failed",
        ),
    }
}

fn system_log_error_response(error: &SystemLogError) -> (StatusCode, &'static str) {
    match error {
        SystemLogError::NotFound => (StatusCode::NOT_FOUND, "Log file not found"),
        SystemLogError::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Server log read failed"),
    }
}

fn item_update_error_response(error: &ItemUpdateError) -> (StatusCode, &'static str) {
    match error {
        ItemUpdateError::Store(ItemUpdateStoreError::NotFound)
        | ItemUpdateError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "Item not found")
        }
        ItemUpdateError::Store(ItemUpdateStoreError::InvalidValue) => {
            (StatusCode::BAD_REQUEST, "Invalid item metadata")
        }
        ItemUpdateError::Store(
            ItemUpdateStoreError::InvalidMetadata | ItemUpdateStoreError::Database(_),
        ) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Item metadata persistence failed",
        ),
        ItemUpdateError::BaseItem(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Item persistence failed")
        }
        ItemUpdateError::ServerConfiguration(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Server configuration persistence failed",
        ),
        ItemUpdateError::VirtualFolder(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Library folder persistence failed",
        ),
    }
}

fn item_lookup_error_response(error: &ItemLookupError) -> (StatusCode, &'static str) {
    match error {
        ItemLookupError::NotFound | ItemLookupError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "Item not found")
        }
        ItemLookupError::BaseItem(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Item lookup data could not be loaded",
        ),
        ItemLookupError::VirtualFolder(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Library options could not be loaded",
        ),
        ItemLookupError::LinkedChild(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Collection members could not be loaded",
        ),
        ItemLookupError::Metadata(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "TMDB metadata provider failed",
        ),
        ItemLookupError::GoogleBooks(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Google Books metadata provider failed",
        ),
        ItemLookupError::TvMaze(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "TV Maze metadata provider failed",
        ),
        ItemLookupError::MusicBrainz(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "MusicBrainz metadata provider failed",
        ),
    }
}

fn media_stream_error_response(error: &MediaStreamServiceError) -> (StatusCode, &'static str) {
    match error {
        MediaStreamServiceError::Store(
            jellyfin_data::MediaStreamStoreError::BaseItemNotFound { .. },
        ) => (StatusCode::NOT_FOUND, "Media stream item not found"),
        MediaStreamServiceError::Store(
            jellyfin_data::MediaStreamStoreError::DuplicateStreamIndex { .. }
            | jellyfin_data::MediaStreamStoreError::InvalidStreamType(_),
        ) => (StatusCode::BAD_REQUEST, "Invalid media stream"),
        MediaStreamServiceError::Store(jellyfin_data::MediaStreamStoreError::Database(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Media stream persistence failed",
        ),
    }
}

fn media_attachment_error_response(
    error: &MediaAttachmentServiceError,
) -> (StatusCode, &'static str) {
    match error {
        MediaAttachmentServiceError::Store(
            jellyfin_data::MediaAttachmentStoreError::BaseItemNotFound { .. },
        ) => (StatusCode::NOT_FOUND, "Media attachment item not found"),
        MediaAttachmentServiceError::Store(
            jellyfin_data::MediaAttachmentStoreError::DuplicateAttachmentIndex { .. },
        ) => (StatusCode::BAD_REQUEST, "Invalid media attachment"),
        MediaAttachmentServiceError::Store(jellyfin_data::MediaAttachmentStoreError::Database(
            _,
        )) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Media attachment persistence failed",
        ),
    }
}

fn metadata_editor_error_response(error: &MetadataEditorError) -> (StatusCode, &'static str) {
    match error {
        MetadataEditorError::NotFound | MetadataEditorError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "Item not found")
        }
        MetadataEditorError::BaseItem(_)
        | MetadataEditorError::ServerConfiguration(_)
        | MetadataEditorError::VirtualFolder(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Metadata editor data could not be loaded",
        ),
    }
}

fn user_error_response(error: &UserError) -> (StatusCode, &'static str) {
    match error {
        UserError::InvalidUsername => (StatusCode::BAD_REQUEST, "Invalid username"),
        UserError::DuplicateUsername(_) => (
            StatusCode::BAD_REQUEST,
            "A user with that name already exists",
        ),
        UserError::NotFound => (StatusCode::NOT_FOUND, "User not found"),
        UserError::PasswordAlreadyConfigured => {
            (StatusCode::FORBIDDEN, "Password is already configured")
        }
        UserError::LastUser => (StatusCode::FORBIDDEN, "There must be at least one user"),
        UserError::LastAdministrator => (
            StatusCode::FORBIDDEN,
            "There must be at least one administrator",
        ),
        UserError::AdministratorCannotBeDisabled => (
            StatusCode::FORBIDDEN,
            "Administrator accounts cannot be disabled",
        ),
        UserError::LastEnabledUser => (
            StatusCode::FORBIDDEN,
            "There must be at least one enabled user",
        ),
        UserError::InvalidPolicy => (StatusCode::BAD_REQUEST, "Invalid user policy"),
        UserError::AdministratorPasswordRequired => (
            StatusCode::FORBIDDEN,
            "Administrator passwords must not be empty",
        ),
        UserError::PasswordResetPinNotFound => {
            (StatusCode::NOT_FOUND, "Password reset PIN not found")
        }
        UserError::ConfigurationSerialization(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "User configuration serialization failed",
        ),
        UserError::PolicySerialization(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "User policy serialization failed",
        ),
        UserError::CorruptPlaylistShares => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Playlist persistence failed",
        ),
        UserError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database operation failed",
        ),
    }
}

fn display_preference_error_response(
    error: &DisplayPreferenceStoreError,
) -> (StatusCode, &'static str) {
    match error {
        DisplayPreferenceStoreError::EmptyField(_)
        | DisplayPreferenceStoreError::FieldTooLong { .. }
        | DisplayPreferenceStoreError::InvalidPreferences => {
            (StatusCode::BAD_REQUEST, "Invalid display preferences")
        }
        DisplayPreferenceStoreError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Display preferences persistence failed",
        ),
    }
}

fn tuner_host_error_response(error: &TunerHostError) -> (StatusCode, &'static str) {
    match error {
        TunerHostError::UnsupportedType | TunerHostError::SourceUnavailable => {
            (StatusCode::NOT_FOUND, "Tuner host provider was not found")
        }
        TunerHostError::Store(jellyfin_data::TunerHostStoreError::InvalidNumericValue) => {
            (StatusCode::BAD_REQUEST, "Invalid tuner host request")
        }
        TunerHostError::Store(jellyfin_data::TunerHostStoreError::Database(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Tuner host persistence failed",
        ),
    }
}

fn guide_refresh_error_response(error: &GuideRefreshError) -> (StatusCode, &'static str) {
    match error {
        GuideRefreshError::NoProvider
        | GuideRefreshError::InvalidProviderConfiguration
        | GuideRefreshError::MissingToken => {
            (StatusCode::NOT_FOUND, "Live TV guide refresh failed")
        }
        GuideRefreshError::Configuration(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Live TV guide configuration failed",
        ),
        GuideRefreshError::Client(_) => (
            StatusCode::BAD_GATEWAY,
            "Schedules Direct guide refresh failed",
        ),
    }
}

fn dashboard_error_response(error: &DashboardError) -> (StatusCode, &'static str) {
    match error {
        DashboardError::NotFound => (StatusCode::NOT_FOUND, "Dashboard page not found"),
        DashboardError::Io(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Dashboard page read failed",
        ),
    }
}

fn video_error_response(error: &VideoError) -> (StatusCode, &'static str) {
    match error {
        VideoError::NotFound | VideoError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "Video not found")
        }
        VideoError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
        VideoError::NotEnoughVideos => (StatusCode::BAD_REQUEST, "Not enough videos to merge"),
        VideoError::BaseItem(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Video persistence failed",
        ),
    }
}

fn year_error_response(error: &YearError) -> (StatusCode, &'static str) {
    match error {
        YearError::InvalidYear => (StatusCode::BAD_REQUEST, "Invalid year"),
        YearError::NotFound
        | YearError::UserNotFound
        | YearError::User(UserError::NotFound)
        | YearError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "Year or user not found")
        }
        YearError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
        YearError::BaseItem(_) | YearError::User(_) | YearError::ItemByName(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Year persistence failed")
        }
    }
}

fn library_controller_error_response(error: &LibraryControllerError) -> (StatusCode, &'static str) {
    match error {
        LibraryControllerError::InvalidRequest => (StatusCode::BAD_REQUEST, "Invalid request"),
        LibraryControllerError::UserNotFound
        | LibraryControllerError::ItemNotFound
        | LibraryControllerError::FileNotFound
        | LibraryControllerError::User(UserError::NotFound)
        | LibraryControllerError::BaseItem(BaseItemError::NotFound)
        | LibraryControllerError::UserLibrary(
            UserLibraryError::UserNotFound
            | UserLibraryError::ItemNotFound
            | UserLibraryError::User(UserError::NotFound)
            | UserLibraryError::BaseItem(BaseItemError::NotFound),
        ) => (StatusCode::NOT_FOUND, "User, item, or file not found"),
        LibraryControllerError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
        LibraryControllerError::Forbidden
        | LibraryControllerError::BaseItem(BaseItemError::ProtectedItem)
        | LibraryControllerError::UserLibrary(
            UserLibraryError::Forbidden | UserLibraryError::BaseItem(BaseItemError::ProtectedItem),
        ) => (StatusCode::FORBIDDEN, "Forbidden"),
        LibraryControllerError::NotDownloadable => {
            (StatusCode::BAD_REQUEST, "Item does not support downloading")
        }
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Library persistence failed",
        ),
    }
}

fn virtual_folder_error_response(error: &VirtualFolderServiceError) -> (StatusCode, &'static str) {
    match error {
        VirtualFolderServiceError::InvalidOptions
        | VirtualFolderServiceError::InvalidCollectionType
        | VirtualFolderServiceError::InvalidPath
        | VirtualFolderServiceError::PathNotDirectory
        | VirtualFolderServiceError::NonUtf8Path
        | VirtualFolderServiceError::Repository(jellyfin_data::VirtualFolderError::InvalidName) => {
            (StatusCode::BAD_REQUEST, "Invalid virtual folder request")
        }
        VirtualFolderServiceError::PathNotFound
        | VirtualFolderServiceError::Repository(
            jellyfin_data::VirtualFolderError::NotFound
            | jellyfin_data::VirtualFolderError::PathNotFound,
        ) => (
            StatusCode::NOT_FOUND,
            "Virtual folder or media path not found",
        ),
        VirtualFolderServiceError::Repository(
            jellyfin_data::VirtualFolderError::DuplicateName
            | jellyfin_data::VirtualFolderError::PathOverlap,
        ) => (
            StatusCode::CONFLICT,
            "Virtual folder or media path already exists",
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Virtual folder persistence failed",
        ),
    }
}
