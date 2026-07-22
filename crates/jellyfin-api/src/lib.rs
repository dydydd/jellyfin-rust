use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_controller::{
    PlaystateError, PlaystateService, UserError, UserLibraryError, UserLibraryService, UserService,
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
mod playstate;
mod startup;
mod user_library;
mod users;

pub use branding::BrandingOptions;

#[derive(Clone)]
pub struct AppState {
    pub(crate) users: UserService,
    pub(crate) activity_logs: ActivityLogRepository,
    pub(crate) devices: DeviceRepository,
    pub(crate) playstate: PlaystateService,
    pub(crate) user_library: UserLibraryService,
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
            user_library: UserLibraryService::new(database.clone()),
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
        .route("/Items/Root", get(user_library::get_root))
        .route("/Items/{item_id}", get(user_library::get_item))
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
        .with_state(Arc::new(state))
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
    UserLibrary(UserLibraryError),
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

impl From<UserLibraryError> for ApiError {
    fn from(error: UserLibraryError) -> Self {
        Self::UserLibrary(error)
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
            Self::UserLibrary(UserLibraryError::Forbidden) => (StatusCode::FORBIDDEN, "Forbidden"),
            Self::Playstate(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Playstate persistence failed",
            ),
            Self::UserLibrary(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Library persistence failed",
            ),
        };
        (status, Json(serde_json::json!({ "Message": message }))).into_response()
    }
}
