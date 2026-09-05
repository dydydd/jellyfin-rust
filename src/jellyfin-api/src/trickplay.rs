use std::{path::Path as FilePath, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Request, Uri, header},
    response::Response,
};
use axum_extra::extract::Query;
use chrono::{DateTime, Utc};
use jellyfin_data::BaseItemError;
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication::AuthenticatedIdentity, authorization};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TrickplayQuery {
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
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
    mut headers: HeaderMap,
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
    let mut request = Request::get("/")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    if headers.contains_key(header::RANGE)
        && if_range_allows(&path, headers.get(header::IF_RANGE)).await
        && let Some(value) = headers.remove(header::RANGE)
    {
        request.headers_mut().insert(header::RANGE, value);
    }
    let response = match ServeFile::new(&path)
        .with_buf_chunk_size(64 * 1024)
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let mut response = response.map(Body::new);
    if response.status().is_success() {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    Ok(response)
}

async fn if_range_allows(path: &FilePath, if_range: Option<&HeaderValue>) -> bool {
    let Some(if_range) = if_range else {
        return true;
    };
    let Ok(if_range) = if_range.to_str() else {
        return false;
    };
    let Ok(if_range_date) = DateTime::parse_from_rfc2822(if_range) else {
        return false;
    };
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    DateTime::<Utc>::from(modified).timestamp() <= if_range_date.timestamp()
}
