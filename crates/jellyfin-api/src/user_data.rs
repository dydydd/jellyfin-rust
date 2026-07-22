use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
};
use jellyfin_model::UserItemDataDto;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
pub struct UserDataQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
}

pub(crate) async fn mark_favorite_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_favorite(state, &uri, headers, query.user_id, item_id, true).await
}

pub(crate) async fn unmark_favorite_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_favorite(state, &uri, headers, query.user_id, item_id, false).await
}

pub(crate) async fn mark_favorite_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_favorite(state, &uri, headers, Some(user_id), item_id, true).await
}

pub(crate) async fn unmark_favorite_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_favorite(state, &uri, headers, Some(user_id), item_id, false).await
}

async fn set_favorite(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    is_favorite: bool,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let update = state
        .user_data
        .set_favorite_for_authorized_user(target_user_id, item_id, is_favorite)
        .await?;
    Ok(Json(update.into()))
}
