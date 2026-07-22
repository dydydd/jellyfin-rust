use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::entities::user;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct StartupConfiguration {
    pub server_name: Option<String>,
    #[serde(rename = "UICulture")]
    pub ui_culture: Option<String>,
    pub metadata_country_code: Option<String>,
    pub preferred_metadata_language: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct StartupUser {
    pub name: Option<String>,
    pub password: Option<String>,
}

pub(crate) struct StartupState {
    pub(crate) configuration: StartupConfiguration,
    pub(crate) completed: bool,
    pub(crate) user_id: Option<Uuid>,
}

impl StartupState {
    pub(crate) fn new(server_name: String) -> Self {
        Self {
            configuration: StartupConfiguration {
                server_name: Some(server_name),
                ..StartupConfiguration::default()
            },
            completed: false,
            user_id: None,
        }
    }
}

pub(crate) async fn get_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<StartupConfiguration>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let startup = state.startup.lock().await;
    Ok(Json(startup.configuration.clone()))
}

pub(crate) async fn update_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<StartupConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let mut startup = state.startup.lock().await;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    startup.configuration = StartupConfiguration {
        server_name: Some(request.server_name.unwrap_or_default()),
        ui_culture: Some(request.ui_culture.unwrap_or_default()),
        metadata_country_code: Some(request.metadata_country_code.unwrap_or_default()),
        preferred_metadata_language: Some(request.preferred_metadata_language.unwrap_or_default()),
    };
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn get_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<StartupUser>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let mut startup = state.startup.lock().await;
    let user = resolve_user(&state, &mut startup).await?;
    Ok(Json(StartupUser {
        name: Some(user.username),
        password: None,
    }))
}

pub(crate) async fn update_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<StartupUser>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let mut startup = state.startup.lock().await;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let mut user = resolve_user(&state, &mut startup).await?;

    if user
        .password_hash
        .as_deref()
        .is_some_and(|password| !password.is_empty())
    {
        return Err(ApiError::Forbidden);
    }
    let password = request
        .password
        .filter(|password| !password.trim().is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let name = request.name.unwrap_or_else(|| user.username.clone());
    let authentication = state.authentication;
    let user = tokio::task::spawn_blocking(move || {
        authentication.change_password(&mut user, &password);
        user
    })
    .await
    .map_err(|_| ApiError::Internal)?;
    let password_hash = user
        .password_hash
        .as_deref()
        .expect("a nonempty password always produces a hash");
    let updated = state
        .users
        .configure_startup_user(user.id, &name, password_hash)
        .await?;
    startup.user_id = Some(updated.id);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn complete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let mut startup = state.startup.lock().await;
    startup.completed = true;
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_user(
    state: &AppState,
    startup: &mut StartupState,
) -> Result<user::Model, ApiError> {
    let user = if let Some(user_id) = startup.user_id {
        state.users.get(user_id).await?
    } else {
        state
            .users
            .first()
            .await?
            .ok_or(ApiError::User(jellyfin_controller::UserError::NotFound))?
    };
    startup.user_id = Some(user.id);
    Ok(user)
}
