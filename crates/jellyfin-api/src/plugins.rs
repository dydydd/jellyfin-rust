use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};

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
