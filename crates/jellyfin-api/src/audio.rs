use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request},
    response::Response,
};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamQuery {
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
}

pub(crate) async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, None, query, request).await
}

pub(crate) async fn stream_with_container(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, container)): Path<(Uuid, String)>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, Some(&container), query, request).await
}

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: StreamQuery,
    mut request: Request<Body>,
) -> Result<Response, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let item = state
        .library_controller
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let path = item
        .path
        .filter(|path| !path.is_empty())
        .ok_or(ApiError::NotFound)?;
    if let Some(container) = requested_container {
        let actual = std::path::Path::new(&path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        if !container.eq_ignore_ascii_case(actual) {
            return Err(ApiError::UnsupportedMediaType);
        }
    }
    if !query.static_stream.unwrap_or(true) {
        return Err(ApiError::UnsupportedMediaType);
    }
    request.headers_mut().clear();
    for name in [
        axum::http::header::RANGE,
        axum::http::header::IF_RANGE,
        axum::http::header::IF_MODIFIED_SINCE,
        axum::http::header::IF_UNMODIFIED_SINCE,
    ] {
        if let Some(value) = headers.get(&name) {
            request.headers_mut().insert(name, value.clone());
        }
    }
    let response = match ServeFile::new(path)
        .with_buf_chunk_size(64 * 1024)
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {},
    };
    Ok(response.map(Body::new))
}
