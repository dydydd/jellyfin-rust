use std::sync::Arc;

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, header},
};
use serde::Serialize;

use crate::{ApiError, AppState, authorization};

const MAX_DOCUMENT_SIZE: usize = 1_000_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ClientLogDocumentResponseDto {
    file_name: String,
}

pub(crate) async fn document(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    request: Request,
) -> Result<(StatusCode, Json<ClientLogDocumentResponseDto>), ApiError> {
    let identity = authorization::require_default(&state, request.headers(), &uri).await?;
    let configuration = state.server_configuration.load().await?;
    if !configuration.allow_client_log_upload {
        return Err(ApiError::Forbidden);
    }
    require_acceptable_size(request.headers())?;

    let (client_name, client_version) = identity.client_log_file_parts(request.headers());
    let body = body_bytes(request.into_body()).await?;
    let mut contents = body.as_ref();
    let file_name = state
        .client_event_logger
        .write_document(&client_name, &client_version, &mut contents)
        .await
        .map_err(|_| ApiError::Internal)?;
    Ok((
        StatusCode::OK,
        Json(ClientLogDocumentResponseDto { file_name }),
    ))
}

fn require_acceptable_size(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|content_length| content_length > MAX_DOCUMENT_SIZE)
    {
        return Err(ApiError::PayloadTooLarge);
    }
    Ok(())
}

async fn body_bytes(body: Body) -> Result<axum::body::Bytes, ApiError> {
    to_bytes(body, MAX_DOCUMENT_SIZE)
        .await
        .map_err(|_| ApiError::PayloadTooLarge)
}
