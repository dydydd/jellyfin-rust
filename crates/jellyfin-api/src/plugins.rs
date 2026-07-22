use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
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
