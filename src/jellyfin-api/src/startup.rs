use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::{
    StartupConfigurationUpdate,
    entities::{server_configuration, user},
};
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct StartupRemoteAccess {
    pub enable_remote_access: bool,
}

pub(crate) struct StartupState {
    pub(crate) configuration: StartupConfiguration,
    pub(crate) completed: bool,
    pub(crate) user_id: Option<Uuid>,
    pub(crate) enable_remote_access: bool,
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
            enable_remote_access: true,
        }
    }
}

pub(crate) struct StartupSnapshot {
    pub(crate) configuration: StartupConfiguration,
    pub(crate) completed: bool,
}

impl From<server_configuration::Model> for StartupSnapshot {
    fn from(configuration: server_configuration::Model) -> Self {
        Self {
            configuration: StartupConfiguration {
                server_name: Some(configuration.server_name),
                ui_culture: Some(configuration.ui_culture),
                metadata_country_code: Some(configuration.metadata_country_code),
                preferred_metadata_language: Some(configuration.preferred_metadata_language),
            },
            completed: configuration.is_startup_wizard_completed,
        }
    }
}

pub(crate) async fn snapshot(state: &AppState) -> Result<StartupSnapshot, ApiError> {
    if let Some(repository) = &state.startup_repository {
        return Ok(repository.load().await?.into());
    }

    let startup = state.startup.lock().await;
    Ok(StartupSnapshot {
        configuration: startup.configuration.clone(),
        completed: startup.completed,
    })
}

pub(crate) async fn is_completed(state: &AppState) -> Result<bool, ApiError> {
    if let Some(repository) = &state.startup_repository {
        return Ok(repository.load().await?.is_startup_wizard_completed);
    }
    Ok(state.startup.lock().await.completed)
}

pub(crate) async fn get_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<StartupConfiguration>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    Ok(Json(snapshot(&state).await?.configuration))
}

pub(crate) async fn update_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<StartupConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let configuration = StartupConfiguration {
        server_name: Some(request.server_name.unwrap_or_default()),
        ui_culture: Some(request.ui_culture.unwrap_or_default()),
        metadata_country_code: Some(request.metadata_country_code.unwrap_or_default()),
        preferred_metadata_language: Some(request.preferred_metadata_language.unwrap_or_default()),
    };
    if let Some(repository) = &state.startup_repository {
        let StartupConfiguration {
            server_name,
            ui_culture,
            metadata_country_code,
            preferred_metadata_language,
        } = configuration;
        repository
            .update_startup_configuration(StartupConfigurationUpdate {
                server_name: server_name.unwrap_or_default(),
                ui_culture: ui_culture.unwrap_or_default(),
                metadata_country_code: metadata_country_code.unwrap_or_default(),
                preferred_metadata_language: preferred_metadata_language.unwrap_or_default(),
            })
            .await?;
    } else {
        state.startup.lock().await.configuration = configuration;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_remote_access(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<StartupRemoteAccess>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    if let Some(repository) = &state.startup_repository {
        repository
            .update_remote_access(request.enable_remote_access)
            .await?;
    } else {
        state.startup.lock().await.enable_remote_access = request.enable_remote_access;
    }
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
    let name = request
        .name
        .unwrap_or_else(|| std::mem::take(&mut user.username));
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
    if let Some(repository) = &state.startup_repository {
        repository.complete_startup().await?;
    } else {
        state.startup.lock().await.completed = true;
    }
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
