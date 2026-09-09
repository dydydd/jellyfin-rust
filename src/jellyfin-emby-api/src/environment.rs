use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use jellyfin_model::FileSystemEntryInfo;
use serde::{Deserialize, Serialize};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Environment/DefaultDirectoryBrowser", get(default_browser))
        .route("/environment/defaultdirectorybrowser", get(default_browser))
        .route(
            "/Environment/DirectoryContents",
            get(directory_contents).post(directory_contents),
        )
        .route(
            "/environment/directorycontents",
            get(directory_contents).post(directory_contents),
        )
        .route("/Environment/Drives", get(drives))
        .route("/environment/drives", get(drives))
        .route("/Environment/NetworkDevices", get(empty_entries))
        .route("/environment/networkdevices", get(empty_entries))
        .route("/Environment/NetworkShares", get(empty_entries))
        .route("/environment/networkshares", get(empty_entries))
        .route("/Environment/ParentPath", get(parent_path))
        .route("/environment/parentpath", get(parent_path))
        .route("/Environment/ValidatePath", post(validate_path))
        .route("/environment/validatepath", post(validate_path))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PathQuery {
    #[serde(alias = "path")]
    path: Option<String>,
    #[serde(alias = "includeFiles")]
    include_files: Option<bool>,
    #[serde(alias = "includeDirectories")]
    include_directories: Option<bool>,
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), Response> {
    state.require_emby_administrator(headers, uri).await
}

async fn default_browser(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<DefaultDirectoryBrowserInfo>, Response> {
    authorize(&state, &headers, &uri).await?;
    Ok(Json(DefaultDirectoryBrowserInfo { path: None }))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct DefaultDirectoryBrowserInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

async fn directory_contents(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    Ok(Json(state.environment_directory_contents(
        &path,
        query.include_files.unwrap_or(false),
        query.include_directories.unwrap_or(false),
    )?))
}

async fn drives(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    Ok(Json(state.environment_drives()))
}

async fn empty_entries(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    Ok(Json(Vec::new()))
}

async fn parent_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    Ok(state.environment_parent_path(&path).map_or_else(
        || StatusCode::NO_CONTENT.into_response(),
        |parent| Json(parent).into_response(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ValidatePath {
    #[serde(alias = "isFile")]
    is_file: Option<bool>,
    #[serde(
        alias = "ValidateWriteable",
        alias = "validateWriteable",
        alias = "validateWritable"
    )]
    validate_writable: Option<bool>,
}

async fn validate_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
    Json(body): Json<ValidatePath>,
) -> Result<StatusCode, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    state.environment_validate_path(
        Some(&path),
        body.is_file,
        body.validate_writable.unwrap_or(false),
    )?;
    Ok(StatusCode::NO_CONTENT)
}
