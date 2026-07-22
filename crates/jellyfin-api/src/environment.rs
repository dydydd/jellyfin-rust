use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use jellyfin_model::FileSystemEntryInfo;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
struct DirectoryContentsQuery {
    #[serde(rename = "path", alias = "Path")]
    path: Option<String>,
    #[serde(default, rename = "includeFiles", alias = "IncludeFiles")]
    include_files: bool,
    #[serde(default, rename = "includeDirectories", alias = "IncludeDirectories")]
    include_directories: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ValidatePathDto {
    #[serde(rename = "ValidateWritable", alias = "validateWritable")]
    validate_writable: bool,
    #[serde(rename = "Path", alias = "path")]
    path: Option<String>,
    #[serde(rename = "IsFile", alias = "isFile")]
    is_file: Option<bool>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct DefaultDirectoryBrowserInfoDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

pub(crate) async fn directory_contents(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FileSystemEntryInfo>>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let query: DirectoryContentsQuery = query(&uri)?;
    let path = query
        .path
        .filter(|path| !path.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let environment = state.environment;
    let entries = tokio::task::spawn_blocking(move || {
        environment.directory_contents(&path, query.include_files, query.include_directories)
    })
    .await
    .map_err(|_| ApiError::Internal)??;
    Ok(Json(entries))
}

pub(crate) async fn validate_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<ValidatePathDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let request = request.map_err(|error| json_error(&error))?.0;
    let environment = state.environment;
    tokio::task::spawn_blocking(move || {
        environment.validate_path(
            request.path.as_deref(),
            request.is_file,
            request.validate_writable,
        )
    })
    .await
    .map_err(|_| ApiError::Internal)??;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn drives(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FileSystemEntryInfo>>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let environment = state.environment;
    let drives = tokio::task::spawn_blocking(move || environment.drives())
        .await
        .map_err(|_| ApiError::Internal)?;
    Ok(Json(drives))
}

pub(crate) async fn parent_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let query: ParentPathQuery = query(&uri)?;
    let path = query
        .path
        .filter(|path| !path.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    Ok(match state.environment.parent_path(&path) {
        Some(parent) => Json(parent).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

#[derive(Debug, Default, Deserialize)]
struct ParentPathQuery {
    #[serde(rename = "path", alias = "Path")]
    path: Option<String>,
}

pub(crate) async fn default_directory_browser(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<DefaultDirectoryBrowserInfoDto>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    Ok(Json(DefaultDirectoryBrowserInfoDto::default()))
}

fn query<T: DeserializeOwned>(uri: &Uri) -> Result<T, ApiError> {
    Query::<T>::try_from_uri(uri)
        .map(|query| query.0)
        .map_err(|_| ApiError::InvalidRequest)
}

fn json_error(error: &JsonRejection) -> ApiError {
    if matches!(error, JsonRejection::MissingJsonContentType(_)) {
        ApiError::UnsupportedMediaType
    } else {
        ApiError::InvalidRequest
    }
}
