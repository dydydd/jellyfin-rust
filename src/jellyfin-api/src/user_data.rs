use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::HeaderMap,
};
use jellyfin_model::{UpdateUserItemDataDto, UserItemDataDto};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
pub struct UserDataQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RatingQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
    #[serde(default, alias = "Likes")]
    pub likes: Option<bool>,
}

pub(crate) async fn get_item_data_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    get_item_data(state, &uri, headers, query.user_id, item_id).await
}

pub(crate) async fn get_item_data_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    get_item_data(state, &uri, headers, Some(user_id), item_id).await
}

async fn get_item_data(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let bypass_preference_gate = identity.is_administrator_equivalent();
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let update = state
        .user_data
        .get_item_data_for_authorized_user(target_user_id, item_id, bypass_preference_gate)
        .await?;
    Ok(Json(update.into()))
}

pub(crate) async fn update_item_data_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserDataQuery>,
    request: Result<Json<UpdateUserItemDataDto>, JsonRejection>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    update_item_data(state, &uri, headers, query.user_id, item_id, request).await
}

pub(crate) async fn update_item_data_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    request: Result<Json<UpdateUserItemDataDto>, JsonRejection>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    update_item_data(state, &uri, headers, Some(user_id), item_id, request).await
}

async fn update_item_data(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    request: Result<Json<UpdateUserItemDataDto>, JsonRejection>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let bypass_preference_gate = identity.is_administrator_equivalent();
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let Json(update) = request.map_err(|error| {
        if matches!(error, JsonRejection::MissingJsonContentType(_)) {
            ApiError::UnsupportedMediaType
        } else {
            ApiError::InvalidRequest
        }
    })?;
    let update = state
        .user_data
        .update_item_data_for_authorized_user(
            target_user_id,
            item_id,
            bypass_preference_gate,
            update,
        )
        .await?;
    let dto: UserItemDataDto = update.into();
    crate::websocket::broadcast_user_data_changed(
        &state,
        target_user_id,
        item_id,
        &serde_json::to_value(&dto).unwrap_or_default(),
    )
    .await;
    Ok(Json(dto))
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
    let dto: UserItemDataDto = update.into();
    crate::websocket::broadcast_user_data_changed(
        &state,
        target_user_id,
        item_id,
        &serde_json::to_value(&dto).unwrap_or_default(),
    )
    .await;
    Ok(Json(dto))
}

pub(crate) async fn set_rating_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<RatingQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_rating(state, &uri, headers, query.user_id, item_id, query.likes).await
}

pub(crate) async fn delete_rating_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserDataQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_rating(state, &uri, headers, query.user_id, item_id, None).await
}

pub(crate) async fn set_rating_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<RatingQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_rating(state, &uri, headers, Some(user_id), item_id, query.likes).await
}

pub(crate) async fn delete_rating_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    set_rating(state, &uri, headers, Some(user_id), item_id, None).await
}

async fn set_rating(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    likes: Option<bool>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let update = state
        .user_data
        .set_rating_for_authorized_user(target_user_id, item_id, likes)
        .await?;
    let dto: UserItemDataDto = update.into();
    crate::websocket::broadcast_user_data_changed(
        &state,
        target_user_id,
        item_id,
        &serde_json::to_value(&dto).unwrap_or_default(),
    )
    .await;
    Ok(Json(dto))
}
