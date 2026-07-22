use std::sync::Arc;

use axum::{
    body::Body,
    extract::{OriginalUri, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, Response, header},
};
use serde::Deserialize;
use tokio_util::io::ReaderStream;

use crate::{ApiError, AppState, authentication};

const STREAM_BUFFER_SIZE: usize = 64 * 1024;
const TEXT_UTF8: HeaderValue = HeaderValue::from_static("text/plain; charset=utf-8");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct LogFileQuery {
    #[serde(alias = "Name")]
    name: Option<String>,
}

pub(crate) async fn get_log_file(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<LogFileQuery>, QueryRejection>,
) -> Result<Response<Body>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    identity.require_administrator()?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let name = query
        .name
        .filter(|name| !name.trim().is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let log = state.system_logs.open(&name).await?;
    let stream = ReaderStream::with_capacity(log.into_file(), STREAM_BUFFER_SIZE);

    Response::builder()
        .header(header::CONTENT_TYPE, TEXT_UTF8)
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError::Internal)
}
