use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_controller::{UserError, UserService};
use jellyfin_data::entities::user;
use jellyfin_model::{PublicSystemInfo, UserConfiguration, UserDto, UserPolicy};
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    users: UserService,
    system_info: PublicSystemInfo,
    database: DatabaseConnection,
}

impl AppState {
    pub fn new(database: DatabaseConnection, server_name: String, local_address: String) -> Self {
        Self {
            users: UserService::new(database.clone()),
            system_info: PublicSystemInfo {
                local_address: Some(local_address),
                server_name: Some(server_name),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                product_name: Some("Jellyfin Server".to_owned()),
                id: Some(Uuid::new_v4().simple().to_string()),
                startup_wizard_completed: Some(false),
                ..PublicSystemInfo::default()
            },
            database,
        }
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
        .with_state(Arc::new(state))
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    match jellyfin_data::healthcheck(&state.database).await {
        Ok(()) => (StatusCode::OK, "Healthy").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Unhealthy").into_response(),
    }
}

async fn public_system_info(State(state): State<Arc<AppState>>) -> Json<PublicSystemInfo> {
    Json(state.system_info.clone())
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

fn user_to_dto(user: user::Model) -> UserDto {
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
enum ApiError {
    User(UserError),
    InvalidRequest,
}

impl From<UserError> for ApiError {
    fn from(error: UserError) -> Self {
        Self::User(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidRequest => (StatusCode::BAD_REQUEST, "Invalid request body"),
            Self::User(UserError::InvalidUsername) => (StatusCode::BAD_REQUEST, "Invalid username"),
            Self::User(UserError::DuplicateUsername(_)) => (
                StatusCode::BAD_REQUEST,
                "A user with that name already exists",
            ),
            Self::User(UserError::NotFound) => (StatusCode::NOT_FOUND, "User not found"),
            Self::User(UserError::Database(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database operation failed",
            ),
        };
        (status, Json(serde_json::json!({ "Message": message }))).into_response()
    }
}
