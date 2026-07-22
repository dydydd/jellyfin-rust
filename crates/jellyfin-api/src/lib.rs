use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_controller::{UserError, UserService};
use jellyfin_data::{AuthenticationStoreError, DeviceRepository, entities::user};
use jellyfin_model::{PublicSystemInfo, UserConfiguration, UserDto, UserPolicy};
use jellyfin_server_implementations::{AuthenticationError, DefaultAuthenticationProvider};
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use tokio::sync::Mutex;
use uuid::Uuid;

mod authentication;
mod startup;

#[derive(Clone)]
pub struct AppState {
    pub(crate) users: UserService,
    pub(crate) devices: DeviceRepository,
    pub(crate) authentication: DefaultAuthenticationProvider,
    pub(crate) system_info: PublicSystemInfo,
    pub(crate) startup: Arc<Mutex<startup::StartupState>>,
    pub(crate) database: DatabaseConnection,
}

impl AppState {
    pub fn new(database: DatabaseConnection, server_name: String, local_address: String) -> Self {
        Self {
            users: UserService::new(database.clone()),
            devices: DeviceRepository::new(database.clone()),
            authentication: DefaultAuthenticationProvider::new(),
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

    pub(crate) fn server_id(&self) -> &str {
        self.system_info.id.as_deref().unwrap_or_default()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/System/Info/Public", get(public_system_info))
        .route("/System/Ping", get(ping).post(ping))
        .route("/Users", get(list_users))
        .route("/Users/Public", get(list_public_users))
        .route("/Users/New", post(create_user))
        .route("/Users/{id}", get(get_user))
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

async fn list_users(State(state): State<Arc<AppState>>) -> Result<Json<Vec<UserDto>>, ApiError> {
    Ok(Json(
        state
            .users
            .list()
            .await?
            .into_iter()
            .map(user_to_dto)
            .collect(),
    ))
}

async fn list_public_users(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    Ok(Json(
        state
            .users
            .list_public()
            .await?
            .into_iter()
            .map(user_to_dto)
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateUserByName {
    name: String,
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    request: Result<Json<CreateUserByName>, JsonRejection>,
) -> Result<Json<UserDto>, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(user_to_dto(state.users.create(&request.name).await?)))
}

async fn get_user(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, ApiError> {
    Ok(Json(user_to_dto(state.users.get(id).await?)))
}

pub(crate) fn user_to_dto(user: user::Model) -> UserDto {
    let mut policy: UserPolicy = serde_json::from_value(user.policy).unwrap_or_default();
    policy.is_administrator = user.is_administrator;
    policy.is_hidden = user.is_hidden;
    policy.is_disabled = user.is_disabled;
    let configuration: UserConfiguration =
        serde_json::from_value(user.preferences).unwrap_or_default();

    UserDto {
        id: user.id,
        name: Some(user.username),
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
    User(UserError),
    Authentication(AuthenticationError),
    AuthenticationStore(AuthenticationStoreError),
    InvalidRequest,
    Unauthorized,
    Forbidden,
    Internal,
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

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidRequest => (StatusCode::BAD_REQUEST, "Invalid request body"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "Forbidden"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error"),
            Self::User(UserError::InvalidUsername) => (StatusCode::BAD_REQUEST, "Invalid username"),
            Self::User(UserError::DuplicateUsername(_)) => (
                StatusCode::BAD_REQUEST,
                "A user with that name already exists",
            ),
            Self::User(UserError::NotFound) => (StatusCode::NOT_FOUND, "User not found"),
            Self::User(UserError::PasswordAlreadyConfigured) => {
                (StatusCode::FORBIDDEN, "Password is already configured")
            }
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
        };
        (status, Json(serde_json::json!({ "Message": message }))).into_response()
    }
}
