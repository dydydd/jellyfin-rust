use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, header},
    response::Response,
};
use jellyfin_controller::MediaAttachmentFilter;
use jellyfin_data::BaseItemError;
use uuid::Uuid;

use crate::{ApiError, AppState};

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    Path((item_id, _media_source_id, index)): Path<(Uuid, String, i32)>,
) -> Result<Response, ApiError> {
    state
        .base_items
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;

    let attachment = state
        .media_attachments
        .get_media_attachments(MediaAttachmentFilter {
            item_id,
            index: Some(index),
        })
        .await?
        .into_iter()
        .next()
        .ok_or(BaseItemError::NotFound)?;
    let path = attachment
        .delivery_url
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or(BaseItemError::NotFound)?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| BaseItemError::NotFound)?;
    let content_type = attachment
        .mime_type
        .as_deref()
        .filter(|mime_type| !mime_type.trim().is_empty())
        .unwrap_or("application/octet-stream");
    let content_type = HeaderValue::from_str(content_type).map_err(|_| ApiError::Internal)?;
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    Ok(response)
}
