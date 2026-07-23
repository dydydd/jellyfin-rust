use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::{ServerConfigurationUpdate, entities::server_configuration};
use jellyfin_model::{MetadataOptions, NameValuePair, RepositoryInfo, ServerConfiguration};

use crate::{ApiError, AppState, authentication, authorization};

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<ServerConfiguration>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let configuration = state.server_configuration.load().await?;
    Ok(Json(server_configuration(configuration)?))
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<ServerConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(configuration) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .server_configuration
        .update_server_configuration(server_configuration_update(configuration)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn default_metadata_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<MetadataOptions>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(MetadataOptions::default()))
}

fn server_configuration(
    model: server_configuration::Model,
) -> Result<ServerConfiguration, ApiError> {
    Ok(ServerConfiguration {
        is_startup_wizard_completed: model.is_startup_wizard_completed,
        preferred_metadata_language: model.preferred_metadata_language,
        metadata_country_code: model.metadata_country_code,
        min_resume_pct: model.min_resume_pct,
        max_resume_pct: model.max_resume_pct,
        min_resume_duration_seconds: model.min_resume_duration_seconds,
        min_audiobook_resume: model.min_audiobook_resume,
        max_audiobook_resume: model.max_audiobook_resume,
        server_name: model.server_name,
        ui_culture: model.ui_culture,
        content_types: serde_json::from_value::<Vec<NameValuePair>>(model.content_types)
            .map_err(|_| ApiError::Internal)?,
        plugin_repositories: serde_json::from_value::<Vec<RepositoryInfo>>(
            model.plugin_repositories,
        )
        .map_err(|_| ApiError::Internal)?,
        allow_client_log_upload: model.allow_client_log_upload,
        ..ServerConfiguration::default()
    })
}

fn server_configuration_update(
    configuration: ServerConfiguration,
) -> Result<ServerConfigurationUpdate, ApiError> {
    Ok(ServerConfigurationUpdate {
        server_name: configuration.server_name,
        ui_culture: configuration.ui_culture,
        metadata_country_code: configuration.metadata_country_code,
        preferred_metadata_language: configuration.preferred_metadata_language,
        is_startup_wizard_completed: configuration.is_startup_wizard_completed,
        content_types: serde_json::to_value(configuration.content_types)
            .map_err(|_| ApiError::Internal)?,
        plugin_repositories: serde_json::to_value(configuration.plugin_repositories)
            .map_err(|_| ApiError::Internal)?,
        min_resume_pct: configuration.min_resume_pct,
        max_resume_pct: configuration.max_resume_pct,
        min_resume_duration_seconds: configuration.min_resume_duration_seconds,
        min_audiobook_resume: configuration.min_audiobook_resume,
        max_audiobook_resume: configuration.max_audiobook_resume,
        allow_client_log_upload: configuration.allow_client_log_upload,
    })
}
