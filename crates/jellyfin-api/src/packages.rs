use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_model::{PackageInfo, RepositoryInfo};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PackageQuery {
    #[serde(rename = "assemblyGuid", alias = "AssemblyGuid")]
    assembly_guid: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct InstallPackageQuery {
    #[serde(rename = "assemblyGuid", alias = "AssemblyGuid")]
    assembly_guid: Option<Uuid>,
    version: Option<String>,
    #[serde(rename = "repositoryUrl", alias = "RepositoryUrl")]
    repository_url: Option<String>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<PackageInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.packages.list()))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<PackageQuery>,
) -> Result<Json<PackageInfo>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.packages.get(&name, query.assembly_guid)?))
}

pub(crate) async fn install(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<InstallPackageQuery>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    state.packages.install_candidate(
        &name,
        query.assembly_guid,
        query.version.as_deref(),
        query.repository_url.as_deref(),
    )?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn cancel_installation(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(_package_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn repositories(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<RepositoryInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    repository_info(&state).await.map(Json)
}

pub(crate) async fn set_repositories(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<Vec<RepositoryInfo>>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Json(repositories) = request.map_err(|_| ApiError::InvalidRequest)?;
    let value = serde_json::to_value(&repositories).map_err(|_| ApiError::Internal)?;
    state
        .server_configuration
        .update_plugin_repositories(value)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}

async fn repository_info(state: &AppState) -> Result<Vec<RepositoryInfo>, ApiError> {
    let configuration = state.server_configuration.load().await?;
    serde_json::from_value(configuration.plugin_repositories).map_err(|_| ApiError::Internal)
}
