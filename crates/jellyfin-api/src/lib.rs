use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use jellyfin_controller::{
    ArtistError, ArtistService, CollectionError, CollectionService, DashboardError, DashboardPage,
    DashboardService, EnvironmentError, EnvironmentService, GenreError, GenreService,
    InstalledPlugin, ItemImageError, ItemImageService, ItemLookupError, ItemLookupService,
    ItemUpdateError, ItemUpdateService, LibraryControllerError, LibraryControllerService,
    LibraryScanError, LibraryScanService, LocalizationService, MediaAttachmentService,
    MediaAttachmentServiceError, MediaSegmentError, MediaSegmentManagerService, MediaStreamService,
    MediaStreamServiceError, MetadataEditorError, MetadataEditorService, MetadataRefreshService,
    MusicGenreError, MusicGenreService, PackageError, PackageService, PersonError, PersonService,
    PlaylistError, PlaylistService, PlaystateError, PlaystateService, PluginRegistry,
    PostgresSessionStore, ScheduledTaskError, ScheduledTaskService, SearchManager, SearchProvider,
    StudioError, StudioService, SubtitleManager, SubtitleProvider, SystemLogError,
    SystemLogService, SystemStorageService, TranscodeJobRegistry, TrickplayError, TrickplayService,
    UserDataService, UserDataServiceError, UserError, UserLibraryError, UserLibraryService,
    UserService, UserViewManagerError, UserViewManagerService, VideoError, VideoService,
    VirtualFolderService, VirtualFolderServiceError, YearError, YearService,
    client_event::ClientEventLogger,
};
use jellyfin_data::{
    ActivityLogError, ActivityLogRepository, ApiKeyRepository, AuthenticationStoreError,
    BaseItemError, BaseItemImageRepository, BaseItemRepository, DeviceOptionsRepository,
    DeviceRepository, DisplayPreferenceRepository, DisplayPreferenceStoreError,
    ItemUpdateStoreError, ItemValueRepository, NamedConfigurationRepository,
    NamedConfigurationStoreError, PersonRepository, QuickConnectRepository,
    ServerConfigurationRepository, ServerConfigurationStoreError, SessionCommandRepository,
    SessionCommandStoreError, entities::user,
};
use jellyfin_drawing::{ImageProcessingError, ImageProcessor};
use jellyfin_live_tv::{
    listings::{GuideRefreshError, GuideRefreshService},
    tuner_hosts::{TunerHostError, TunerHostManager},
};
use jellyfin_media_encoding::encoder::EncoderCapabilities;
use jellyfin_model::{PublicSystemInfo, UserConfiguration, UserDto, UserPolicy};
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

mod activity_log;
mod api_keys;
mod artists;
mod audio;
mod authentication;
mod authorization;
mod backup;
mod branding;
mod channels;
mod client_log;
mod collections;
mod configuration;
mod dashboard;
mod devices;
mod display_preferences;
mod environment;
mod filters;
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

pub use branding::BrandingOptions;

/// Host lifecycle commands exposed by the system API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemCommand {
    Restart,
    Shutdown,
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
        QuickConnectManager<jellyfin_server_implementations::SystemQuickConnectCapability>,
    pub(crate) quick_connect_capability: SystemQuickConnectCapability,
    pub(crate) playstate: PlaystateService,
    pub(crate) playlists: PlaylistService,
    pub(crate) collections: CollectionService,
    pub(crate) user_data: UserDataService,
    pub(crate) artists: ArtistService,
    pub(crate) genres: GenreService,
    pub(crate) studios: StudioService,
    pub(crate) music_genres: MusicGenreService,
    pub(crate) persons: PersonService,
    pub(crate) item_images: ItemImageService,
    pub(crate) metadata_refresh: MetadataRefreshService,
    pub(crate) base_items: BaseItemRepository,
    pub(crate) item_values: ItemValueRepository,
    pub(crate) people: PersonRepository,
    pub(crate) base_item_images: BaseItemImageRepository,
    pub(crate) dto_images: PersistedDtoImageProjectionService<ItemImageService>,
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
    pub(crate) live_tv_guide: Option<GuideRefreshService>,
    pub(crate) virtual_folders: VirtualFolderService,
    pub(crate) user_views: UserViewManagerService,
    pub(crate) dashboard: DashboardService,
    pub(crate) environment: EnvironmentService,
    pub(crate) plugins: PluginRegistry,
    pub(crate) packages: PackageService,
    pub(crate) scheduled_tasks: ScheduledTaskService,
    pub(crate) library_scan: LibraryScanService,
    pub(crate) system_logs: SystemLogService,
    pub(crate) system_storage: SystemStorageService,
    pub(crate) trickplay: TrickplayService,
    pub(crate) client_event_logger: ClientEventLogger,
    pub(crate) named_configurations: Option<NamedConfigurationRepository>,
    pub(crate) program_data_directory: PathBuf,
    pub(crate) web_directory: PathBuf,
    pub(crate) image_cache_directory: PathBuf,
    pub(crate) cache_directory: PathBuf,
    pub(crate) internal_metadata_directory: PathBuf,
    pub(crate) network_manager: Arc<NetworkManager>,
    pub(crate) transcode_directory: PathBuf,
    pub(crate) ffmpeg_path: PathBuf,
    pub(crate) encoder_capabilities: EncoderCapabilities,
    pub(crate) transcode_jobs: TranscodeJobRegistry,
    pub(crate) authentication: DefaultAuthenticationProvider,
    pub(crate) session_manager: SessionManager<PostgresSessionStore>,
    pub(crate) branding: Arc<tokio::sync::RwLock<BrandingOptions>>,
    pub(crate) system_info: PublicSystemInfo,
    pub(crate) startup: Arc<Mutex<startup::StartupState>>,
    pub(crate) startup_repository: Option<ServerConfigurationRepository>,
    pub(crate) database: DatabaseConnection,
    pub(crate) tmdb_api_key: Arc<tokio::sync::RwLock<String>>,
    pub(crate) omdb_api_key: Arc<tokio::sync::RwLock<String>>,
    pub(crate) system_command: Arc<dyn Fn(SystemCommand) + Send + Sync>,
}

impl AppState {
    #[allow(clippy::too_many_lines)]
    pub fn new(database: DatabaseConnection, server_name: String, local_address: String) -> Self {
        let library_scan = LibraryScanService::new(database.clone());
        let scheduled_tasks = ScheduledTaskService::with_default_executors(library_scan.clone());
        let item_images = ItemImageService::new(database.clone());
        let metadata_refresh =
            MetadataRefreshService::new(database.clone(), Some(item_images.clone()));
        let base_items = BaseItemRepository::new(database.clone());
        let item_values = ItemValueRepository::new(database.clone());
        let people = PersonRepository::new(database.clone());
        let base_item_images = BaseItemImageRepository::new(database.clone());
        let web_sockets = Arc::new(websocket::WebSocketHub::new());
        let quick_connect_capability = SystemQuickConnectCapability::new(true);
        let user_library = UserLibraryService::new(database.clone());
        let search = SearchManager::with_default_database(user_library.clone());
        let session_store = PostgresSessionStore::new(
            UserService::new(database.clone()),
            DeviceRepository::new(database.clone()),
            ActivityLogRepository::new(database.clone()),
            DefaultAuthenticationProvider::new(),
        );
        let state = Self {
            users: UserService::new(database.clone()),
            activity_logs: ActivityLogRepository::new(database.clone()),
            api_keys: ApiKeyRepository::new(database.clone()),
            devices: DeviceRepository::new(database.clone()),
            device_options: DeviceOptionsRepository::new(database.clone()),
            display_preferences: DisplayPreferenceRepository::new(database.clone()),
            session_commands: SessionCommandRepository::new(database.clone()),
            sync_play: SyncPlayManager::new(),
            web_sockets: web_sockets.clone(),
            quick_connect: QuickConnectManager::new(
                QuickConnectRepository::new(database.clone()),
                quick_connect_capability.clone(),
            ),
            quick_connect_capability,
            playstate: PlaystateService::new(database.clone()),
            playlists: PlaylistService::new(database.clone()),
            collections: CollectionService::new(database.clone()),
            user_data: UserDataService::new(database.clone()),
            artists: ArtistService::new(database.clone()),
            genres: GenreService::new(database.clone()),
            studios: StudioService::new(database.clone()),
            music_genres: MusicGenreService::new(database.clone()),
            persons: PersonService::new(database.clone()),
            dto_images: PersistedDtoImageProjectionService::new(
                base_items.clone(),
                base_item_images.clone(),
                item_images.clone(),
            ),
            item_images,
            metadata_refresh,
            base_items,
            item_values,
            people,
            base_item_images,
            image_processor: ImageProcessor::with_concurrency::<4>(
                PathBuf::from("cache").join("images"),
            ),
            item_lookup: ItemLookupService::new(database.clone()),
            item_update: ItemUpdateService::new(database.clone()),
            metadata_editor: MetadataEditorService::new(database.clone()),
            localization: LocalizationService,
            server_configuration: ServerConfigurationRepository::new(database.clone()),
            user_library,
            search,
            library_controller: LibraryControllerService::new(database.clone()),
            media_attachments: MediaAttachmentService::new(database.clone()),
            media_segments: MediaSegmentManagerService::new(database.clone()),
            media_streams: MediaStreamService::new(database.clone()),
            subtitles: SubtitleManager::default(),
            videos: VideoService::new(database.clone()),
            years: YearService::new(database.clone()),
            tuner_hosts: TunerHostManager::new(database.clone()),
            live_tv_guide: None,
            virtual_folders: VirtualFolderService::new(database.clone()),
            user_views: UserViewManagerService::new(database.clone()),
            dashboard: DashboardService::default(),
            environment: EnvironmentService::new(),
            plugins: PluginRegistry::default(),
            packages: PackageService::default(),
            scheduled_tasks: scheduled_tasks.clone(),
            library_scan,
            system_logs: SystemLogService::default(),
            system_storage: SystemStorageService::new(),
            trickplay: TrickplayService::new(
                database.clone(),
                PathBuf::from("programdata").join("trickplay"),
            ),
            client_event_logger: ClientEventLogger::new("logs"),
            named_configurations: if matches!(database, DatabaseConnection::Disconnected) {
                None
            } else {
                Some(NamedConfigurationRepository::new(database.clone()))
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
            transcode_directory: std::env::temp_dir()
                .join("jellyfin-rust")
                .join("transcodes"),
            ffmpeg_path: PathBuf::from("ffmpeg"),
            encoder_capabilities: EncoderCapabilities::default(),
            transcode_jobs: TranscodeJobRegistry::new(),
            authentication: DefaultAuthenticationProvider::new(),
            session_manager: SessionManager::new(session_store),
            branding: Arc::new(tokio::sync::RwLock::new(BrandingOptions::default())),
            system_info: PublicSystemInfo {
                local_address: Some(local_address),
                server_name: Some(server_name.clone()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                product_name: Some("Jellyfin Server".to_owned()),
                id: Some(Uuid::new_v4().simple().to_string()),
                startup_wizard_completed: Some(false),
                ..PublicSystemInfo::default()
            },
            startup: Arc::new(Mutex::new(startup::StartupState::new(server_name))),
            startup_repository: None,
            database,
            tmdb_api_key: Arc::new(tokio::sync::RwLock::new(String::new())),
            omdb_api_key: Arc::new(tokio::sync::RwLock::new("2c9d9507".to_owned())),
            system_command: Arc::new(|_| {}),
        };
        state.scheduled_tasks.add_change_listener(Arc::new(move || {
            let tasks = scheduled_tasks.clone();
            let sockets = web_sockets.clone();
            tokio::spawn(async move {
                let infos = tasks.list(None, None).await;
                sockets.send_all("ScheduledTasksInfo", &infos).await;
            });
        }));
        state.scheduled_tasks.start_scheduler();
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
    #[must_use]
    pub fn with_tmdb_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.tmdb_api_key = Arc::new(tokio::sync::RwLock::new(api_key.into()));
        self
    }

    /// Replaces the `OMDb` API key used by metadata providers.
    #[must_use]
    pub fn with_omdb_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.omdb_api_key = Arc::new(tokio::sync::RwLock::new(api_key.into()));
        self
    }

    /// Replaces the Quick Connect availability used by authentication.
    #[must_use]
    pub fn with_quick_connect_available(self, available: bool) -> Self {
        self.quick_connect_capability.set_enabled(available);
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
        self.library_scan
            .set_image_cache_directory(self.image_cache_directory.as_path());
        self.item_images.set_storage_directories(
            self.image_cache_directory.as_path(),
            self.internal_metadata_directory.as_path(),
        );
        self.metadata_refresh
            .set_images(Some(self.item_images.clone()));
        self.dto_images = PersistedDtoImageProjectionService::new(
            self.base_items.clone(),
            self.base_item_images.clone(),
            self.item_images.clone(),
        );
        self.image_processor =
            ImageProcessor::with_concurrency::<4>(self.image_cache_directory.as_path());
        self.trickplay
            .set_storage_directory(self.program_data_directory.join("trickplay"));
        self.cache_directory = cache_directory.into();
        self.scheduled_tasks
            .set_cache_directory(self.cache_directory.as_path());
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
        self.live_tv_guide = Some(service);
        self
    }

    /// Replaces the directory containing active transcoding output.
    #[must_use]
    pub fn with_transcode_directory(
        mut self,
        transcode_directory: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.transcode_directory = transcode_directory.into();
        self.scheduled_tasks
            .set_transcode_directory(self.transcode_directory.as_path());
        self
    }

    /// Replaces the `FFmpeg` binary used for transcoding.
    #[must_use]
    pub fn with_ffmpeg_path(mut self, ffmpeg_path: impl Into<std::path::PathBuf>) -> Self {
        self.ffmpeg_path = ffmpeg_path.into();
        self.library_scan
            .set_ffmpeg_path(self.ffmpeg_path.as_path());
        self
    }

    /// Replaces the probed encoder capabilities used by transcode decisions.
    #[must_use]
    pub fn with_encoder_capabilities(mut self, capabilities: EncoderCapabilities) -> Self {
        self.encoder_capabilities = capabilities;
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
                .flat_map(|folder| folder.locations.into_iter().map(PathBuf::from))
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot list libraries for the watcher");
                Vec::new()
            }
        };
        if let Err(error) = jellyfin_controller::library_watcher::LibraryWatcher::new(
            self.library_scan.clone(),
            self.virtual_folders.clone(),
            paths,
        )
        .start()
        {
            tracing::error!(%error, "library watcher failed to start");
        }
        self
    }

    pub(crate) fn server_id(&self) -> &str {
        self.system_info.id.as_deref().unwrap_or_default()
    }
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::needless_pass_by_value)]
pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let base = base_router(state.clone());

    Router::new()
        .nest("/api", base.clone())
        .nest("/emby", base.clone())
        .merge(base)
        .with_state(state)
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
        .route("/Branding/Css", get(branding::get_css))
        .route("/Branding/Css.css", get(branding::get_css))
        .route(
            "/Branding/Splashscreen",
            get(branding::get_splashscreen)
                .post(branding::upload_splashscreen)
                .delete(branding::delete_splashscreen),
        )
        .route("/Channels", get(channels::list))
        .route("/Channels/Features", get(channels::all_features))
        .route(
            "/Channels/Items/Latest",
            get(channels::latest_channel_items),
        )
        .route(
            "/Channels/{channel_id}/Features",
            get(channels::features),
        )
        .route(
            "/Channels/{channel_id}/Items",
            get(channels::channel_items),
        )
        .route(
            "/Artists/{name}/Images/{image_type}/{image_index}",
            get(artists::get_image),
        )
        .route("/Search/Hints", get(search::hints))
        .route("/Backup", get(backup::list))
        .route("/Backup/Create", post(backup::create))
        .route("/Backup/Manifest", get(backup::manifest))
        .route("/Backup/Restore", post(backup::restore))
        .route("/Items/{item_id}/Images", get(item_images::list))
        .route(
            "/Items/{item_id}/Images/{image_type}",
            get(item_images::get)
                .post(item_images::upload)
                .delete(item_images::delete),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}",
            get(item_images::get_by_index)
                .post(item_images::upload_by_index)
                .delete(item_images::delete_by_index),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}/Index",
            post(item_images::update_index),
        )
        .route(
            "/Items/{item_id}/Images/{image_type}/{image_index}/{tag}/{format}/{max_width}/{max_height}/{percent_played}/{unplayed_count}",
            get(item_images::get_legacy_path),
        )
        .route("/Items/{item_id}/RemoteImages", get(remote_images::images))
        .route(
            "/Items/{item_id}/RemoteImages/Providers",
            get(remote_images::providers),
        )
        .route(
            "/Items/{item_id}/RemoteImages/Download",
            post(remote_images::download),
        )
        .route(
            "/System/Configuration",
            get(configuration::get).post(configuration::update),
        )
        .route(
            "/System/Configuration/MetadataOptions/Default",
            get(configuration::default_metadata_options),
        )
        .route(
            "/System/Configuration/Branding",
            post(branding::update_configuration),
        )
        .route(
            "/System/Configuration/{key}",
            get(configuration::get_named).post(configuration::update_named),
        )
        .route("/web/ConfigurationPage", get(dashboard::configuration_page))
        .route(
            "/web/ConfigurationPages",
            get(dashboard::configuration_pages),
        )
        .route("/Playback/BitrateTest", get(media_info::bitrate_test))
        .route(
            "/Items/{item_id}/PlaybackInfo",
            get(media_info::get_playback_info).post(media_info::post_playback_info),
        )
        .route("/LiveStreams/Open", post(media_info::open_live_stream))
        .route("/LiveStreams/Close", post(media_info::close_live_stream))
        .route(
            "/MediaSegments/{item_id}",
            get(media_segments::get_item_segments),
        )
        .route("/FallbackFont/Fonts", get(subtitles::fallback_fonts))
        .route("/FallbackFont/Fonts/{name}", get(subtitles::fallback_font))
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
            "/Audio/{item_id}/universal",
            get(audio::universal).head(audio::universal),
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
            "/Videos/{item_id}/stream",
            get(videos::stream).head(videos::stream),
        )
        .route(
            "/Videos/{item_id}/stream.{container}",
            get(videos::stream_with_container).head(videos::stream_with_container),
        )
        .route("/Plugins", get(plugins::list))
        .route(
            "/Plugins/{plugin_id}/{version}/Enable",
            post(plugins::enable),
        )
        .route(
            "/Plugins/{plugin_id}/{version}/Disable",
            post(plugins::disable),
        )
        .route(
            "/Plugins/{plugin_id}/{version}",
            delete(plugins::uninstall_version),
        )
        .route("/Plugins/{plugin_id}", delete(plugins::uninstall))
        .route(
            "/Plugins/{plugin_id}/Configuration",
            get(plugins::get_configuration).post(plugins::update_configuration),
        )
        .route("/Plugins/{plugin_id}/Manifest", post(plugins::manifest))
        .route("/Plugins/{plugin_id}/{version}/Image", get(plugins::image))
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
            "/Users/{user_id}/Items/{item_id}",
            get(user_library::get_item_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Intros",
            get(user_library::get_intros_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/LocalTrailers",
            get(user_library::get_local_trailers_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/SpecialFeatures",
            get(user_library::get_special_features_legacy),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Lyrics",
            get(user_library::get_lyrics_legacy),
        )
        .merge(item_query_routes())
        .merge(library_controller_routes())
        .merge(user_library_routes())
        .merge(video_routes())
        .merge(live_tv_routes())
        .route("/Items/Filters", get(filters::filters_legacy))
        .route("/Items/Filters2", get(filters::filters2))
        .route("/Artists", get(artists::list))
        .route("/Artists/AlbumArtists", get(artists::list_album_artists))
        .route("/Artists/{name}", get(artists::get))
        .route("/Years", get(years::list))
        .route("/Years/{year}", get(years::get))
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
        .route("/Trailers", get(trailers::list))
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
        .route(
            "/Library/VirtualFolders",
            get(virtual_folders::list)
                .post(virtual_folders::create)
                .delete(virtual_folders::delete),
        )
        .route(
            "/Library/VirtualFolders/Name",
            post(virtual_folders::rename),
        )
        .route(
            "/Library/VirtualFolders/Paths",
            post(virtual_folders::add_path).delete(virtual_folders::remove_path),
        )
        .route(
            "/Library/VirtualFolders/Paths/Update",
            post(virtual_folders::update_path),
        )
        .route(
            "/Library/VirtualFolders/LibraryOptions",
            post(virtual_folders::update_options),
        )
        .nest_service(
            "/web",
            ServeDir::new(&state.web_directory).fallback(ServeFile::new(&index_path)),
        )
        .fallback(robots::redirect_or_not_found)
        .with_state(state.clone());

    router.layer(middleware::from_fn_with_state(
        state,
        authorization::require_route_auth,
    ))
}

fn system_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/System/ActivityLog/Entries", get(activity_log::entries))
        .route("/System/Logs", get(system::get_logs))
        .route("/System/Logs/Log", get(system::get_log_file))
        .route("/System/Info", get(system::info))
        .route("/System/Info/Storage", get(system::storage))
        .route("/System/Endpoint", get(system::endpoint_info))
        .route("/System/Restart", post(system::restart))
        .route("/System/Shutdown", post(system::shutdown))
        .route("/Document", post(client_log::document))
        .route("/ClientLog/Document", post(client_log::document))
        .route("/GetUtcTime", get(time_sync::get_utc_time))
        .route("/ScheduledTasks", get(scheduled_tasks::list))
        .route(
            "/ScheduledTasks/Running/{task_id}",
            post(scheduled_tasks::start).delete(scheduled_tasks::stop),
        )
        .route(
            "/ScheduledTasks/{task_id}/Triggers",
            post(scheduled_tasks::update_triggers),
        )
        .route("/ScheduledTasks/{task_id}", get(scheduled_tasks::get))
}

fn sync_play_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/SyncPlay/New", post(sync_play::create_group))
        .route("/SyncPlay/Join", post(sync_play::join_group))
        .route("/SyncPlay/Leave", post(sync_play::leave_group))
        .route("/SyncPlay/List", get(sync_play::list_groups))
        .route("/SyncPlay/SetNewQueue", post(sync_play::set_new_queue))
        .route(
            "/SyncPlay/SetPlaylistItem",
            post(sync_play::set_playlist_item),
        )
        .route(
            "/SyncPlay/RemoveFromPlaylist",
            post(sync_play::remove_from_playlist),
        )
        .route(
            "/SyncPlay/MovePlaylistItem",
            post(sync_play::move_playlist_item),
        )
        .route("/SyncPlay/Queue", post(sync_play::queue_items))
        .route("/SyncPlay/Unpause", post(sync_play::unpause))
        .route("/SyncPlay/Pause", post(sync_play::pause))
        .route("/SyncPlay/Stop", post(sync_play::stop))
        .route("/SyncPlay/Seek", post(sync_play::seek))
        .route("/SyncPlay/Buffering", post(sync_play::buffering))
        .route("/SyncPlay/Ready", post(sync_play::ready))
        .route("/SyncPlay/SetIgnoreWait", post(sync_play::set_ignore_wait))
        .route("/SyncPlay/NextItem", post(sync_play::next_item))
        .route("/SyncPlay/PreviousItem", post(sync_play::previous_item))
        .route("/SyncPlay/SetRepeatMode", post(sync_play::set_repeat_mode))
        .route(
            "/SyncPlay/SetShuffleMode",
            post(sync_play::set_shuffle_mode),
        )
        .route("/SyncPlay/Ping", post(sync_play::ping))
        .route("/SyncPlay/{id}", get(sync_play::get_group))
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
}

fn localization_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Localization/Cultures", get(localization::cultures))
        .route("/Localization/Countries", get(localization::countries))
        .route(
            "/Localization/ParentalRatings",
            get(localization::parental_ratings),
        )
        .route("/Localization/Options", get(localization::options))
}

fn api_key_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Auth/Keys", get(api_keys::list).post(api_keys::create))
        .route("/Auth/Keys/{key}", axum::routing::delete(api_keys::revoke))
}

fn package_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Packages", get(packages::list))
        .route("/Packages/Installed/{name}", post(packages::install))
        .route(
            "/Packages/Installing/{package_id}",
            axum::routing::delete(packages::cancel_installation),
        )
        .route("/Packages/{name}", get(packages::get))
        .route(
            "/Repositories",
            get(packages::repositories).post(packages::set_repositories),
        )
}

fn startup_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Startup/Configuration",
            get(startup::get_configuration).post(startup::update_configuration),
        )
        .route("/Startup/RemoteAccess", post(startup::update_remote_access))
        .route(
            "/Startup/User",
            get(startup::get_user).post(startup::update_user),
        )
        .route("/Startup/FirstUser", get(startup::get_user))
        .route("/Startup/Complete", post(startup::complete))
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
            "/Users/AuthenticateWithQuickConnect",
            post(authentication::authenticate_with_quick_connect),
        )
        .route(
            "/Users/{user_id}/Authenticate",
            post(authentication::authenticate),
        )
        .route("/Users/Me", get(authentication::current_user))
}

fn quick_connect_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/QuickConnect/Enabled", get(quick_connect::enabled))
        .route("/QuickConnect/Initiate", post(quick_connect::initiate))
        .route("/QuickConnect/Connect", get(quick_connect::connect))
        .route("/QuickConnect/Authorize", post(quick_connect::authorize))
}

fn device_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Devices", get(devices::list).delete(devices::delete))
        .route("/Devices/Info", get(devices::info))
        .route(
            "/Devices/Options",
            get(devices::options).post(devices::update_options),
        )
}

fn display_preference_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/DisplayPreferences/{display_preferences_id}",
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
        .route("/Users", get(users::list).post(users::update))
        .route("/Users/Public", get(users::list_public))
        .route("/Users/New", post(users::create))
        .route("/Users/ForgotPassword", post(users::forgot_password))
        .route(
            "/Users/ForgotPassword/Pin",
            post(users::forgot_password_pin),
        )
        .route("/Users/Configuration", post(users::update_configuration))
        .route(
            "/Users/{id}",
            get(users::get)
                .post(users::update_legacy)
                .delete(users::delete),
        )
        .route("/User/{id}", axum::routing::delete(users::delete))
        .route("/Users/Password", post(users::update_password_query))
        .route(
            "/Users/{id}/Configuration",
            post(users::update_configuration_legacy),
        )
        .route(
            "/Users/{id}/Images/{image_type}",
            get(users::get_user_image_legacy)
                .post(users::post_user_image_legacy)
                .delete(users::delete_user_image_legacy),
        )
        .route(
            "/Users/{id}/Images/{image_type}/{index}",
            get(users::get_user_image_index_legacy)
                .post(users::post_user_image_index_legacy)
                .delete(users::delete_user_image_index_legacy),
        )
        .route("/Users/{id}/Password", post(users::update_password))
        .route("/Users/{id}/Policy", post(users::update_policy))
}

fn user_view_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/UserViews", get(user_views::get))
        .route(
            "/UserViews/GroupingOptions",
            get(user_views::grouping_options),
        )
        .route("/Users/{user_id}/Views", get(user_views::get_legacy))
        .route(
            "/Users/{user_id}/GroupingOptions",
            get(user_views::grouping_options_legacy),
        )
}

fn session_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sessions", get(session::list))
        .route(
            "/Sessions/{session_id}/System/{command}",
            post(session::send_system_command),
        )
        .route(
            "/Sessions/{session_id}/Viewing",
            post(session::display_content),
        )
        .route(
            "/Sessions/{session_id}/Playing",
            post(session::send_play_command),
        )
        .route(
            "/Sessions/{session_id}/Playing/{command}",
            post(session::send_playstate_command),
        )
        .route(
            "/Sessions/{session_id}/Command/{command}",
            post(session::send_general_command),
        )
        .route(
            "/Sessions/{session_id}/Command",
            post(session::send_full_general_command),
        )
        .route(
            "/Sessions/{session_id}/Message",
            post(session::send_message_command),
        )
        .route(
            "/Sessions/{session_id}/User/{user_id}",
            post(session::add_user_to_session).delete(session::remove_user_from_session),
        )
        .route("/Sessions/Viewing", post(session::report_viewing))
        .route("/Sessions/Capabilities", post(session::post_capabilities))
        .route(
            "/Sessions/Capabilities/Full",
            post(session::post_full_capabilities),
        )
        .route("/Sessions/Logout", post(session::logout))
        .route("/Auth/Providers", get(session::authentication_providers))
        .route(
            "/Auth/PasswordResetProviders",
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
            "/UserPlayedItems/{item_id}",
            post(playstate::mark_played_modern).delete(playstate::mark_unplayed_modern),
        )
        .route(
            "/Users/{user_id}/PlayedItems/{item_id}",
            post(playstate::mark_played).delete(playstate::mark_unplayed),
        )
}

fn user_data_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/UserItems/{item_id}/UserData",
            get(user_data::get_item_data_modern).post(user_data::update_item_data_modern),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/UserData",
            get(user_data::get_item_data_legacy).post(user_data::update_item_data_legacy),
        )
        .route(
            "/UserFavoriteItems/{item_id}",
            post(user_data::mark_favorite_modern).delete(user_data::unmark_favorite_modern),
        )
        .route(
            "/Users/{user_id}/FavoriteItems/{item_id}",
            post(user_data::mark_favorite_legacy).delete(user_data::unmark_favorite_legacy),
        )
        .route(
            "/UserItems/{item_id}/Rating",
            post(user_data::set_rating_modern).delete(user_data::delete_rating_modern),
        )
        .route(
            "/Users/{user_id}/Items/{item_id}/Rating",
            post(user_data::set_rating_legacy).delete(user_data::delete_rating_legacy),
        )
}

fn item_query_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items", get(items::get).delete(library::delete_items))
        .route("/Items/Suggestions", get(items::suggestions))
        .route("/Items/Latest", get(items::latest))
        .route("/UserItems/Resume", get(items::resume))
        .route("/Users/{user_id}/Items", get(items::get_legacy))
        .route(
            "/Users/{user_id}/Suggestions",
            get(items::suggestions_legacy),
        )
        .route("/Users/{user_id}/Items/Latest", get(items::latest_legacy))
        .route("/Users/{user_id}/Items/Resume", get(items::resume_legacy))
}

fn collection_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Collections", post(collections::create))
        .route(
            "/Collections/{collection_id}/Items",
            post(collections::add_items).delete(collections::remove_items),
        )
        .route("/Playlists", post(playlists::create))
        .route(
            "/Playlists/{playlist_id}",
            get(playlists::get).post(playlists::update),
        )
        .route("/Playlists/{playlist_id}/Users", get(playlists::get_users))
        .route(
            "/Playlists/{playlist_id}/Users/{user_id}",
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
            "/Playlists/{playlist_id}/Items/{item_id}/Move/{new_index}",
            post(playlists::move_item),
        )
}

fn library_controller_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Songs/{item_id}/InstantMix", get(library::instant_mix))
        .route("/Albums/{item_id}/InstantMix", get(library::instant_mix))
        .route("/Playlists/{item_id}/InstantMix", get(library::instant_mix))
        .route("/Artists/{item_id}/InstantMix", get(library::instant_mix))
        .route("/Items/{item_id}/InstantMix", get(library::instant_mix))
        .route(
            "/MusicGenres/InstantMix",
            get(library::instant_mix_genre_by_id),
        )
        .route("/Artists/InstantMix", get(library::instant_mix_by_id))
        .route(
            "/MusicGenres/{name}/InstantMix",
            get(library::instant_mix_genre_by_name),
        )
        .route("/Items/Counts", get(library::item_counts))
        .route("/Items/{item_id}/File", get(library::file))
        .route("/Items/{item_id}/ThemeSongs", get(library::theme_songs))
        .route("/Items/{item_id}/ThemeVideos", get(library::theme_videos))
        .route("/Items/{item_id}/ThemeMedia", get(library::theme_media))
        .route("/Items/{item_id}/Ancestors", get(library::ancestors))
        .route("/Items/{item_id}/Download", get(library::download))
        .route("/Items/{item_id}/Collections", get(library::collections))
        .route("/Library/Refresh", post(library::refresh))
        .route("/Library/PhysicalPaths", get(library::physical_paths))
        .route("/Library/MediaFolders", get(library::media_folders))
        .route("/Library/Series/Added", post(library::updated_series))
        .route("/Library/Series/Updated", post(library::updated_series))
        .route("/Library/Movies/Added", post(library::updated_movies))
        .route("/Library/Movies/Updated", post(library::updated_movies))
        .route("/Library/Media/Updated", post(library::updated_media))
        .route(
            "/Libraries/AvailableOptions",
            get(library::available_options),
        )
        .route("/Artists/{item_id}/Similar", get(library::similar))
        .route("/Items/{item_id}/Similar", get(library::similar))
        .route("/Albums/{item_id}/Similar", get(library::similar))
        .route("/Shows/{item_id}/Similar", get(library::similar))
        .route("/Movies/Recommendations", get(movies::recommendations))
        .route("/Movies/{item_id}/Similar", get(library::similar))
        .route("/Shows/NextUp", get(tv_shows::next_up))
        .route("/Shows/Upcoming", get(tv_shows::upcoming))
        .route("/Shows/{series_id}/Episodes", get(tv_shows::episodes))
        .route("/Shows/{series_id}/Seasons", get(tv_shows::seasons))
        .route("/Trailers/{item_id}/Similar", get(library::similar))
}

fn user_library_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/Root", get(user_library::get_root))
        .route(
            "/Items/{item_id}",
            get(user_library::get_item)
                .post(item_update::update)
                .delete(library::delete_item),
        )
        .route(
            "/Items/{item_id}/ContentType",
            post(item_update::update_content_type),
        )
        .route("/Items/{item_id}/Refresh", post(item_refresh::refresh))
        .route(
            "/Items/{item_id}/MetadataEditor",
            get(item_update::metadata_editor),
        )
        .route(
            "/Items/{item_id}/ExternalIdInfos",
            get(item_lookup::external_id_infos),
        )
        .route(
            "/Items/RemoteSearch/Movie",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/Trailer",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/MusicVideo",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/Series",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/BoxSet",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/MusicArtist",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/MusicAlbum",
            post(item_lookup::remote_search),
        )
        .route(
            "/Items/RemoteSearch/Person",
            post(item_lookup::remote_search_elevated),
        )
        .route("/Items/RemoteSearch/Book", post(item_lookup::remote_search))
        .route(
            "/Items/RemoteSearch/Apply/{item_id}",
            post(item_lookup::apply_remote_search),
        )
        .route("/Items/{item_id}/Intros", get(user_library::get_intros))
        .route(
            "/Items/{item_id}/LocalTrailers",
            get(user_library::get_local_trailers),
        )
        .route(
            "/Items/{item_id}/SpecialFeatures",
            get(user_library::get_special_features),
        )
        .route(
            "/Audio/{item_id}/RemoteSearch/Lyrics",
            get(user_library::search_remote_lyrics),
        )
        .route(
            "/Items/{item_id}/RemoteSearch/Subtitles/{id}",
            get(subtitles::search_remote_subtitles).post(subtitles::download_remote_subtitles),
        )
        .route(
            "/Audio/{item_id}/RemoteSearch/Lyrics/{lyric_id}",
            post(user_library::download_remote_lyrics),
        )
        .route(
            "/Audio/{item_id}/Lyrics",
            get(user_library::get_lyrics)
                .post(user_library::upload_lyrics)
                .delete(user_library::delete_lyrics),
        )
        .route(
            "/Providers/Lyrics/{lyric_id}",
            get(user_library::get_remote_lyrics),
        )
        .route(
            "/Providers/Subtitles/Subtitles/{subtitle_id}",
            get(subtitles::get_remote_subtitles),
        )
}

fn video_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Videos/MergeVersions", post(videos::merge_versions))
        .route(
            "/Videos/{item_id}/AlternateSources",
            axum::routing::delete(videos::delete_alternate_sources),
        )
        .route(
            "/Videos/{item_id}/AdditionalParts",
            get(videos::additional_parts),
        )
        .route(
            "/Videos/{item_id}/Subtitles/{index}",
            axum::routing::delete(subtitles::delete_subtitle),
        )
        .route(
            "/Videos/{item_id}/Subtitles",
            post(subtitles::upload_subtitle),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/Stream.{format}",
            get(subtitles::get_subtitle),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/{start_position_ticks}/Stream.{format}",
            get(subtitles::get_subtitle_with_ticks),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/subtitles.m3u8",
            get(subtitles::get_subtitle_playlist),
        )
        .route(
            "/Videos/{item_id}/{media_source_id}/Attachments/{index}",
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

pub(crate) fn user_to_dto_with_server_id(state: &AppState, user: user::Model) -> UserDto {
    let mut dto = user_to_dto(user);
    dto.server_id = Some(state.server_id().to_owned());
    dto
}

pub(crate) fn user_to_dto(user: user::Model) -> UserDto {
    let has_password = user
        .password_hash
        .as_deref()
        .is_some_and(|hash| !hash.is_empty());
    let mut policy: UserPolicy = serde_json::from_value(user.policy).unwrap_or_default();
    policy.is_administrator = user.is_administrator;
    policy.is_hidden = user.is_hidden;
    policy.is_disabled = user.is_disabled;
    policy.authentication_provider_id = Some(user.authentication_provider_id);
    policy.password_reset_provider_id = Some(user.password_reset_provider_id);
    policy.invalid_login_attempt_count = user.invalid_login_attempt_count;
    policy.login_attempts_before_lockout = user.login_attempts_before_lockout;
    let mut configuration: UserConfiguration =
        serde_json::from_value(user.preferences).unwrap_or_default();
    configuration.enable_local_password = user.enable_local_password;

    UserDto {
        id: user.id,
        name: Some(user.username),
        has_password: Some(has_password),
        has_configured_password: Some(has_password),
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
            | Self::Genre(GenreError::Forbidden)
            | Self::Studio(StudioError::Forbidden)
            | Self::MusicGenre(MusicGenreError::Forbidden)
            | Self::Person(PersonError::Forbidden) => (StatusCode::FORBIDDEN, "Forbidden"),
            Self::UserLibrary(UserLibraryError::InvalidPolicy(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stored user policy is invalid",
            ),
            Self::Genre(
                GenreError::NotFound
                | GenreError::UserNotFound
                | GenreError::User(UserError::NotFound)
                | GenreError::BaseItem(BaseItemError::NotFound),
            ) => (StatusCode::NOT_FOUND, "Genre or user not found"),
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
                | PersonError::User(UserError::NotFound)
                | PersonError::BaseItem(BaseItemError::NotFound),
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
        | LibraryScanError::BaseItem(_)
        | LibraryScanError::Chapter(_)
        | LibraryScanError::Keyframe(_)
        | LibraryScanError::ItemImage(_)
        | LibraryScanError::ItemValue(_)
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
            LinkedChildStoreError::Database(_) | LinkedChildStoreError::CorruptChildType(_),
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
            LinkedChildStoreError::CorruptChildType(_) | LinkedChildStoreError::Database(_),
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
        ItemLookupError::Metadata(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "TMDB metadata provider failed",
        ),
        ItemLookupError::GoogleBooks(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Google Books metadata provider failed",
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
        VideoError::InvalidItemType => (StatusCode::BAD_REQUEST, "Item is not a video"),
        VideoError::BaseItem(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Video persistence failed",
        ),
    }
}

fn year_error_response(error: &YearError) -> (StatusCode, &'static str) {
    match error {
        YearError::NotFound
        | YearError::UserNotFound
        | YearError::User(UserError::NotFound)
        | YearError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "Year or user not found")
        }
        YearError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
        YearError::BaseItem(_) | YearError::User(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Year persistence failed")
        }
    }
}

fn library_controller_error_response(error: &LibraryControllerError) -> (StatusCode, &'static str) {
    match error {
        LibraryControllerError::UserNotFound
        | LibraryControllerError::ItemNotFound
        | LibraryControllerError::FileNotFound
        | LibraryControllerError::User(UserError::NotFound)
        | LibraryControllerError::BaseItem(BaseItemError::NotFound) => {
            (StatusCode::NOT_FOUND, "User, item, or file not found")
        }
        LibraryControllerError::Forbidden
        | LibraryControllerError::BaseItem(BaseItemError::ProtectedItem) => {
            (StatusCode::FORBIDDEN, "Forbidden")
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
