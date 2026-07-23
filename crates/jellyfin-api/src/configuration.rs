use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::HeaderMap,
};
use jellyfin_data::entities::server_configuration;
use jellyfin_model::{NameValuePair, RepositoryInfo, ServerConfiguration};

use crate::{ApiError, AppState, authorization};

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<ServerConfiguration>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let configuration = state.server_configuration.load().await?;
    Ok(Json(server_configuration(configuration)?))
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
