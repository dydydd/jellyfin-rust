use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_controller::{
    DashboardError, DashboardPage, DashboardService, EnvironmentError, EnvironmentService,
    InstalledPlugin, ItemLookupError, ItemLookupService, ItemUpdateError, ItemUpdateService,
    LibraryControllerError, LibraryControllerService, LocalizationService, MediaAttachmentService,
    MediaAttachmentServiceError, MediaStreamService, MediaStreamServiceError, MetadataEditorError,
    MetadataEditorService, MusicGenreError, MusicGenreService, PersonError, PersonService,
    PlaystateError, PlaystateService, PluginRegistry, SystemLogError, SystemLogService,
    UserDataService, UserDataServiceError, UserError, UserLibraryError, UserLibraryService,
    UserService, VideoError, VideoService, VirtualFolderService, VirtualFolderServiceError,
};
use jellyfin_data::{
    ActivityLogError, ActivityLogRepository, ApiKeyRepository, AuthenticationStoreError,
    BaseItemError, DeviceRepository, ItemUpdateStoreError, ServerConfigurationRepository,
    ServerConfigurationStoreError, entities::user,
};
use jellyfin_live_tv::tuner_hosts::{TunerHostError, TunerHostManager};
use jellyfin_model::{PublicSystemInfo, UserConfiguration, UserDto, UserPolicy};
use jellyfin_server_implementations::{AuthenticationError, DefaultAuthenticationProvider};
use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;
use uuid::Uuid;

mod activity_log;
mod authentication;
mod authorization;
mod branding;
mod dashboard;
mod environment;
mod hls_segment;
mod item_lookup;
mod item_update;
mod items;
mod library;
mod live_tv;
mod localization;
mod media_info;
mod music_genre;
mod openapi;
mod persons;
mod playstate;
mod plugins;
pub mod query;
mod robots;
mod startup;
mod system;
mod user_data;
mod user_library;
mod users;
mod videos;
mod virtual_folders;

pub use branding::BrandingOptions;

#[derive(Clone)]
pub struct AppState {
    pub(crate) users: UserService,
    pub(crate) activity_logs: ActivityLogRepository,
    pub(crate) api_keys: ApiKeyRepository,
    pub(crate) devices: DeviceRepository,
    pub(crate) playstate: PlaystateService,
    pub(crate) user_data: UserDataService,
    pub(crate) music_genres: MusicGenreService,
    pub(crate) persons: PersonService,
    pub(crate) item_lookup: ItemLookupService,
    pub(crate) item_update: ItemUpdateService,
    pub(crate) metadata_editor: MetadataEditorService,
    pub(crate) localization: LocalizationService,
    pub(crate) server_configuration: ServerConfigurationRepository,
    pub(crate) user_library: UserLibraryService,
    pub(crate) library_controller: LibraryControllerService,
    pub(crate) media_attachments: MediaAttachmentService,
    pub(crate) media_streams: MediaStreamService,
    pub(crate) videos: VideoService,
    pub(crate) tuner_hosts: TunerHostManager,
    pub(crate) virtual_folders: VirtualFolderService,
    pub(crate) dashboard: DashboardService,
    pub(crate) environment: EnvironmentService,
    pub(crate) plugins: PluginRegistry,
    pub(crate) system_logs: SystemLogService,
    pub(crate) transcode_directory: std::path::PathBuf,
    pub(crate) authentication: DefaultAuthenticationProvider,
    pub(crate) branding: Arc<tokio::sync::RwLock<BrandingOptions>>,
    pub(crate) system_info: PublicSystemInfo,
    pub(crate) startup: Arc<Mutex<startup::StartupState>>,
    pub(crate) startup_repository: Option<ServerConfigurationRepository>,
    pub(crate) database: DatabaseConnection,
}

impl AppState {
    pub fn new(database: DatabaseConnection, server_name: String, local_address: String) -> Self {
        Self {
            users: UserService::new(database.clone()),
            activity_logs: ActivityLogRepository::new(database.clone()),
            api_keys: ApiKeyRepository::new(database.clone()),
            devices: DeviceRepository::new(database.clone()),
            playstate: PlaystateService::new(database.clone()),
            user_data: UserDataService::new(database.clone()),
            music_genres: MusicGenreService::new(database.clone()),
            persons: PersonService::new(database.clone()),
            item_lookup: ItemLookupService::new(database.clone()),
            item_update: ItemUpdateService::new(database.clone()),
            metadata_editor: MetadataEditorService::new(database.clone()),
            localization: LocalizationService,
            server_configuration: ServerConfigurationRepository::new(database.clone()),
            user_library: UserLibraryService::new(database.clone()),
            library_controller: LibraryControllerService::new(database.clone()),
            media_attachments: MediaAttachmentService::new(database.clone()),
            media_streams: MediaStreamService::new(database.clone()),
            videos: VideoService::new(database.clone()),
            tuner_hosts: TunerHostManager::new(database.clone()),
            virtual_folders: VirtualFolderService::new(database.clone()),
            dashboard: DashboardService::default(),
            environment: EnvironmentService::new(),
            plugins: PluginRegistry::default(),
            system_logs: SystemLogService::default(),
            transcode_directory: std::env::temp_dir()
                .join("jellyfin-rust")
                .join("transcodes"),
            authentication: DefaultAuthenticationProvider::new(),
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
        }
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

    /// Replaces the branding configuration used by the public branding API.
    #[must_use]
    pub fn with_branding_options(mut self, branding: BrandingOptions) -> Self {
        self.branding = Arc::new(tokio::sync::RwLock::new(branding));
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
        self.system_logs = SystemLogService::new(log_directory);
        self
    }

    /// Replaces the directory containing active transcoding output.
    #[must_use]
    pub fn with_transcode_directory(
        mut self,
        transcode_directory: impl Into<std::path::PathBuf>,
    ) -> Self {
        self.transcode_directory = transcode_directory.into();
        self
    }

    pub(crate) fn server_id(&self) -> &str {
        self.system_info.id.as_deref().unwrap_or_default()
    }
}

pub fn router(state: AppState) -> Router {
    openapi::documented_routes()
        .route("/System/ActivityLog/Entries", get(activity_log::entries))
        .route("/System/Logs", get(system::get_logs))
        .route("/System/Logs/Log", get(system::get_log_file))
        .route("/Branding/Configuration", get(branding::get_configuration))
        .route("/Branding/Css", get(branding::get_css))
        .route("/Branding/Css.css", get(branding::get_css))
        .route("/web/ConfigurationPage", get(dashboard::configuration_page))
        .route(
            "/web/ConfigurationPages",
            get(dashboard::configuration_pages),
        )
        .route("/Playback/BitrateTest", get(media_info::bitrate_test))
        .route(
            "/Audio/{item_id}/hls/{*legacy_path}",
            get(hls_segment::audio),
        )
        .route(
            "/Videos/{item_id}/hls/{*legacy_path}",
            get(hls_segment::video),
        )
        .route("/Plugins", get(plugins::list))
        .route("/Plugins/{plugin_id}/{version}/Image", get(plugins::image))
        .merge(environment_routes())
        .merge(localization_routes())
        .merge(user_routes())
        .route(
            "/Startup/Configuration",
            get(startup::get_configuration).post(startup::update_configuration),
        )
        .route(
            "/Startup/User",
            get(startup::get_user).post(startup::update_user),
        )
        .route("/Startup/FirstUser", get(startup::get_user))
        .route("/Startup/Complete", post(startup::complete))
        .route(
            "/Users/AuthenticateByName",
            post(authentication::authenticate_by_name),
        )
        .route("/Users/Me", get(authentication::current_user))
        .merge(playstate_routes())
        .merge(user_data_routes())
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
        .route("/MusicGenres/{genre_name}", get(music_genre::get))
        .route("/Persons/{name}", get(persons::get))
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
        .fallback(robots::redirect_or_not_found)
        .with_state(Arc::new(state))
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

fn user_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users", get(users::list).post(users::update))
        .route("/Users/Public", get(users::list_public))
        .route("/Users/New", post(users::create))
        .route(
            "/Users/{id}",
            get(users::get)
                .post(users::update_legacy)
                .delete(users::delete),
        )
        .route("/User/{id}", axum::routing::delete(users::delete))
        .route("/Users/Password", post(users::update_password_query))
        .route("/Users/{id}/Password", post(users::update_password))
        .route("/Users/{id}/Policy", post(users::update_policy))
}

fn playstate_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sessions/Playing", post(playstate::report_playback_start))
        .route(
            "/Sessions/Playing/Progress",
            post(playstate::report_playback_progress),
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
        .route("/UserItems/Resume", get(items::resume))
        .route("/Users/{user_id}/Items", get(items::get_legacy))
        .route("/Users/{user_id}/Items/Resume", get(items::resume_legacy))
}

fn library_controller_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/{item_id}/File", get(library::file))
        .route("/Items/{item_id}/ThemeSongs", get(library::theme_songs))
        .route("/Items/{item_id}/ThemeVideos", get(library::theme_videos))
        .route("/Items/{item_id}/ThemeMedia", get(library::theme_media))
        .route("/Items/{item_id}/Ancestors", get(library::ancestors))
        .route("/Items/{item_id}/Download", get(library::download))
        .route("/Items/{item_id}/Collections", get(library::collections))
        .route("/Artists/{item_id}/Similar", get(library::similar))
        .route("/Items/{item_id}/Similar", get(library::similar))
        .route("/Albums/{item_id}/Similar", get(library::similar))
        .route("/Shows/{item_id}/Similar", get(library::similar))
        .route("/Movies/{item_id}/Similar", get(library::similar))
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
        .route(
            "/Items/{item_id}/MetadataEditor",
            get(item_update::metadata_editor),
        )
        .route(
            "/Items/{item_id}/ExternalIdInfos",
            get(item_lookup::external_id_infos),
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
        .route("/Audio/{item_id}/Lyrics", get(user_library::get_lyrics))
}

fn video_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/Videos/{item_id}/AlternateSources",
        axum::routing::delete(videos::delete_alternate_sources),
    )
}

fn live_tv_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/LiveTv/TunerHosts",
        post(live_tv::save_tuner_host).delete(live_tv::delete_tuner_host),
    )
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    match jellyfin_data::healthcheck(&state.database).await {
        Ok(()) => (StatusCode::OK, "Healthy").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Unhealthy").into_response(),
    }
}

async fn public_system_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicSystemInfo>, ApiError> {
    let startup = startup::snapshot(&state).await?;
    let mut system_info = state.system_info.clone();
    system_info
        .server_name
        .clone_from(&startup.configuration.server_name);
    system_info.startup_wizard_completed = Some(startup.completed);
    Ok(Json(system_info))
}

async fn ping() -> &'static str {
    "Jellyfin Server"
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
    let configuration: UserConfiguration =
        serde_json::from_value(user.preferences).unwrap_or_default();

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
    Playstate(PlaystateError),
    UserData(UserDataServiceError),
    MusicGenre(MusicGenreError),
    Person(PersonError),
    UserLibrary(UserLibraryError),
    LibraryController(LibraryControllerError),
    Video(VideoError),
    VirtualFolder(VirtualFolderServiceError),
    Dashboard(DashboardError),
    TunerHost(TunerHostError),
    ItemLookup(ItemLookupError),
    ItemUpdate(ItemUpdateError),
    MediaAttachment(MediaAttachmentServiceError),
    MediaStream(MediaStreamServiceError),
    MetadataEditor(MetadataEditorError),
    SystemLog(SystemLogError),
    ServerConfiguration(ServerConfigurationStoreError),
    Environment(EnvironmentError),
    InvalidRequest,
    UnsupportedMediaType,
    Unauthorized,
    Forbidden,
    Internal,
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

impl From<VirtualFolderServiceError> for ApiError {
    fn from(error: VirtualFolderServiceError) -> Self {
        Self::VirtualFolder(error)
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

impl From<ItemUpdateError> for ApiError {
    fn from(error: ItemUpdateError) -> Self {
        Self::ItemUpdate(error)
    }
}

impl From<MediaAttachmentServiceError> for ApiError {
    fn from(error: MediaAttachmentServiceError) -> Self {
        Self::MediaAttachment(error)
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

impl From<ServerConfigurationStoreError> for ApiError {
    fn from(error: ServerConfigurationStoreError) -> Self {
        Self::ServerConfiguration(error)
    }
}

impl From<EnvironmentError> for ApiError {
    fn from(error: EnvironmentError) -> Self {
        Self::Environment(error)
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
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            Self::Forbidden
            | Self::Playstate(PlaystateError::Forbidden)
            | Self::UserData(UserDataServiceError::Forbidden) => {
                (StatusCode::FORBIDDEN, "Forbidden")
            }
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
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
            | Self::MusicGenre(MusicGenreError::Forbidden)
            | Self::Person(PersonError::Forbidden) => (StatusCode::FORBIDDEN, "Forbidden"),
            Self::MusicGenre(
                MusicGenreError::NotFound
                | MusicGenreError::UserNotFound
                | MusicGenreError::User(UserError::NotFound),
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
            Self::Dashboard(error) => dashboard_error_response(&error),
            Self::TunerHost(error) => tuner_host_error_response(&error),
            Self::ItemLookup(error) => item_lookup_error_response(&error),
            Self::ItemUpdate(error) => item_update_error_response(&error),
            Self::MediaAttachment(error) => media_attachment_error_response(&error),
            Self::MediaStream(error) => media_stream_error_response(&error),
            Self::MetadataEditor(error) => metadata_editor_error_response(&error),
            Self::SystemLog(error) => system_log_error_response(&error),
            Self::ServerConfiguration(_error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Startup configuration persistence failed",
            ),
        };
        (status, Json(serde_json::json!({ "Message": message }))).into_response()
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
        UserError::PolicySerialization(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "User policy serialization failed",
        ),
        UserError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Database operation failed",
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
        VideoError::InvalidItemType => (StatusCode::BAD_REQUEST, "Item is not a video"),
        VideoError::BaseItem(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Video persistence failed",
        ),
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
