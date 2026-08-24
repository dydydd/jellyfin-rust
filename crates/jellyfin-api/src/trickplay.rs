use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Uri, header},
    response::Response,
};
use axum_extra::extract::Query;
use jellyfin_data::BaseItemError;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication::AuthenticatedIdentity, authorization};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TrickplayQuery {
    #[serde(rename = "mediaSourceId", alias = "MediaSourceId")]
    media_source_id: Option<Uuid>,
}

pub(crate) async fn playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    Path((item_id, width)): Path<(Uuid, i32)>,
    Query(query): Query<TrickplayQuery>,
) -> Result<Response, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let item_id = query.media_source_id.unwrap_or(item_id);
    let playlist = state
        .trickplay
        .playlist(item_id, width, identity.access_token())
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut response = Response::new(Body::from(playlist));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-mpegURL; charset=utf-8"),
    );
    Ok(response)
}

pub(crate) async fn tile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    Path((item_id, width, tile)): Path<(Uuid, i32, String)>,
    Query(query): Query<TrickplayQuery>,
) -> Result<Response, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let index = tile
        .strip_suffix(".jpg")
        .ok_or(ApiError::InvalidRequest)?
        .parse::<i32>()
        .map_err(|_| ApiError::InvalidRequest)?;
    let item_id = query.media_source_id.unwrap_or(item_id);
    match identity {
        AuthenticatedIdentity::Device(session) => {
            state
                .user_data
                .visible_item(session.user.id, item_id)
                .await?;
        }
        AuthenticatedIdentity::ApiKey(_) => {
            state
                .base_items
                .get(item_id)
                .await?
                .ok_or(BaseItemError::NotFound)?;
        }
    }

    let path = state
        .trickplay
        .tile_path(item_id, width, index)
        .await?
        .ok_or(ApiError::NotFound)?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    Ok(response)
}
