use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use jellyfin_model::PluginInfo;
use serde_json::Value;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

const JSON_UTF8: HeaderValue = HeaderValue::from_static("application/json; charset=utf-8");

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let mut response = Json(state.plugins.plugins()).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, JSON_UTF8);
    Ok(response)
}

pub(crate) async fn image(
    State(state): State<Arc<AppState>>,
    Path((plugin_id, version)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let Some(image) = state.plugins.image(plugin_id, &version) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let request = Request::builder()
        .method("GET")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    let response = match ServeFile::new(image.path).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let mut response = response.map(Body::new);
    if response.status().is_success() {
        let content_type =
            HeaderValue::from_str(&image.mime_type).map_err(|_| ApiError::Internal)?;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    Ok(response)
}

pub(crate) async fn enable(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((plugin_id, version)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    if state
        .plugins
        .enable(plugin_id, &version)
        .map_err(|_| ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

pub(crate) async fn disable(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((plugin_id, version)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    if state
        .plugins
        .disable(plugin_id, &version)
        .map_err(|_| ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

pub(crate) async fn uninstall_version(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((plugin_id, version)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    if state
        .plugins
        .uninstall(plugin_id, Some(&version))
        .map_err(|_| ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

pub(crate) async fn uninstall(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(plugin_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    if state
        .plugins
        .uninstall(plugin_id, None)
        .map_err(|_| ApiError::Internal)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

pub(crate) async fn get_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(plugin_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(
        state
            .plugins
            .configuration(plugin_id)
            .map_err(|_| ApiError::Internal)?
            .ok_or(ApiError::NotFound)?,
    ))
}

pub(crate) async fn update_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(plugin_id): Path<Uuid>,
    request: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Json(_configuration) = request.map_err(|_| ApiError::InvalidRequest)?;
    if state
        .plugins
        .configuration(plugin_id)
        .map_err(|_| ApiError::Internal)?
        .is_some()
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

pub(crate) async fn manifest(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(plugin_id): Path<Uuid>,
) -> Result<Json<PluginInfo>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(
        state
            .plugins
            .manifest(plugin_id)
            .map_err(|_| ApiError::Internal)?
            .ok_or(ApiError::NotFound)?,
    ))
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
