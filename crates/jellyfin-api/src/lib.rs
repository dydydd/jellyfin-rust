use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_controller::{
    DashboardError, DashboardPage, DashboardService, LibraryControllerError,
    LibraryControllerService, MusicGenreError, MusicGenreService, PersonError, PersonService,
    PlaystateError, PlaystateService, PluginRegistry, UserError, UserLibraryError,
    UserLibraryService, UserService, VideoError, VideoService, VirtualFolderService,
    VirtualFolderServiceError,
};
use jellyfin_data::{
    ActivityLogError, ActivityLogRepository, AuthenticationStoreError, BaseItemError,
    DeviceRepository, entities::user,
};
use jellyfin_model::{PublicSystemInfo, UserConfiguration, UserDto, UserPolicy};
use jellyfin_server_implementations::{AuthenticationError, DefaultAuthenticationProvider};
use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;
use uuid::Uuid;

mod activity_log;
mod authentication;
mod branding;
mod dashboard;
mod items;
mod library;
mod media_info;
mod music_genre;
mod persons;
mod playstate;
mod plugins;
mod startup;
mod user_library;
mod users;
mod videos;
mod virtual_folders;

pub use branding::BrandingOptions;

#[derive(Clone)]
pub struct AppState {
    pub(crate) users: UserService,
    pub(crate) activity_logs: ActivityLogRepository,
    pub(crate) devices: DeviceRepository,
    pub(crate) playstate: PlaystateService,
    pub(crate) music_genres: MusicGenreService,
    pub(crate) persons: PersonService,
    pub(crate) user_library: UserLibraryService,
    pub(crate) library_controller: LibraryControllerService,
    pub(crate) videos: VideoService,
    pub(crate) virtual_folders: VirtualFolderService,
    pub(crate) dashboard: DashboardService,
    pub(crate) plugins: PluginRegistry,
    pub(crate) authentication: DefaultAuthenticationProvider,
    pub(crate) branding: Arc<tokio::sync::RwLock<BrandingOptions>>,
    pub(crate) system_info: PublicSystemInfo,
    pub(crate) startup: Arc<Mutex<startup::StartupState>>,
    pub(crate) database: DatabaseConnection,
}

impl AppState {
    pub fn new(database: DatabaseConnection, server_name: String, local_address: String) -> Self {
        Self {
            users: UserService::new(database.clone()),
            activity_logs: ActivityLogRepository::new(database.clone()),
            devices: DeviceRepository::new(database.clone()),
            playstate: PlaystateService::new(database.clone()),
            music_genres: MusicGenreService::new(database.clone()),
            persons: PersonService::new(database.clone()),
            user_library: UserLibraryService::new(database.clone()),
            library_controller: LibraryControllerService::new(database.clone()),
            videos: VideoService::new(database.clone()),
            virtual_folders: VirtualFolderService::new(database.clone()),
            dashboard: DashboardService::default(),
            plugins: PluginRegistry::default(),
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

    pub(crate) fn server_id(&self) -> &str {
        self.system_info.id.as_deref().unwrap_or_default()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/System/Info/Public", get(public_system_info))
        .route("/System/Ping", get(ping).post(ping))
        .route("/System/ActivityLog/Entries", get(activity_log::entries))
        .route("/Branding/Configuration", get(branding::get_configuration))
        .route("/Branding/Css", get(branding::get_css))
        .route("/Branding/Css.css", get(branding::get_css))
        .route("/web/ConfigurationPage", get(dashboard::configuration_page))
        .route(
            "/web/ConfigurationPages",
            get(dashboard::configuration_pages),
        )
        .route("/Playback/BitrateTest", get(media_info::bitrate_test))
        .route("/Plugins", get(plugins::list))
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
        .route(
            "/Startup/Configuration",
            get(startup::get_configuration).post(startup::update_configuration),
        )
        .route(
            "/Startup/User",
            get(startup::get_user).post(startup::update_user),
        )
        .route("/Startup/Complete", post(startup::complete))
        .route(
            "/Users/AuthenticateByName",
            post(authentication::authenticate_by_name),
        )
        .route("/Users/Me", get(authentication::current_user))
        .route(
            "/Users/{user_id}/PlayedItems/{item_id}",
            post(playstate::mark_played).delete(playstate::mark_unplayed),
        )
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
        .with_state(Arc::new(state))
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
            get(user_library::get_item).delete(library::delete_item),
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

async fn health(State(state): State<Arc<AppState>>) -> Response {
    match jellyfin_data::healthcheck(&state.database).await {
        Ok(()) => (StatusCode::OK, "Healthy").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Unhealthy").into_response(),
    }
}

async fn public_system_info(State(state): State<Arc<AppState>>) -> Json<PublicSystemInfo> {
    let startup = state.startup.lock().await;
    let mut system_info = state.system_info.clone();
    system_info
        .server_name
        .clone_from(&startup.configuration.server_name);
    system_info.startup_wizard_completed = Some(startup.completed);
    Json(system_info)
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
    MusicGenre(MusicGenreError),
    Person(PersonError),
    UserLibrary(UserLibraryError),
    LibraryController(LibraryControllerError),
    Video(VideoError),
    VirtualFolder(VirtualFolderServiceError),
    Dashboard(DashboardError),
    InvalidRequest,
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidRequest | Self::Playstate(PlaystateError::InvalidDatePlayed) => {
                (StatusCode::BAD_REQUEST, "Invalid request")
            }
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            Self::Forbidden | Self::Playstate(PlaystateError::Forbidden) => {
                (StatusCode::FORBIDDEN, "Forbidden")
            }
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
            Self::ActivityLog(
                ActivityLogError::EmptyField(_) | ActivityLogError::FieldTooLong { .. },
            ) => (StatusCode::BAD_REQUEST, "Invalid activity log entry"),
            Self::ActivityLog(ActivityLogError::Database(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Activity log persistence failed",
            ),
            Self::User(UserError::InvalidUsername) => (StatusCode::BAD_REQUEST, "Invalid username"),
            Self::User(UserError::DuplicateUsername(_)) => (
                StatusCode::BAD_REQUEST,
                "A user with that name already exists",
            ),
            Self::User(UserError::NotFound) => (StatusCode::NOT_FOUND, "User not found"),
            Self::User(UserError::PasswordAlreadyConfigured) => {
                (StatusCode::FORBIDDEN, "Password is already configured")
            }
            Self::User(UserError::LastUser) => {
                (StatusCode::FORBIDDEN, "There must be at least one user")
            }
            Self::User(UserError::LastAdministrator) => (
                StatusCode::FORBIDDEN,
                "There must be at least one administrator",
            ),
            Self::User(UserError::AdministratorPasswordRequired) => (
                StatusCode::FORBIDDEN,
                "Administrator passwords must not be empty",
            ),
            Self::User(UserError::Database(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database operation failed",
            ),
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
            ) => (StatusCode::NOT_FOUND, "User or item not found"),
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
        };
        (status, Json(serde_json::json!({ "Message": message }))).into_response()
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
