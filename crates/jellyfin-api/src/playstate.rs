use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
};
use jellyfin_controller::{PlaystateUpdate, format_date_played, parse_date_played};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
pub struct MarkPlayedQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
    #[serde(default, rename = "datePlayed", alias = "DatePlayed")]
    pub date_played: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MarkUnplayedQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
}

pub(crate) async fn mark_played_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<MarkPlayedQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_played_for(
        state,
        &uri,
        headers,
        query.user_id,
        item_id,
        query.date_played.as_deref(),
    )
    .await
}

pub(crate) async fn mark_unplayed_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<MarkUnplayedQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_unplayed_for(state, &uri, headers, query.user_id, item_id).await
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub rating: Option<f64>,
    pub played_percentage: Option<f64>,
    pub unplayed_item_count: Option<i32>,
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub likes: Option<bool>,
    pub last_played_date: Option<String>,
    pub played: bool,
    pub key: String,
    pub item_id: String,
}

pub(crate) async fn mark_played(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<MarkPlayedQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_played_for(
        state,
        &uri,
        headers,
        Some(user_id),
        item_id,
        query.date_played.as_deref(),
    )
    .await
}

async fn mark_played_for(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    date_played: Option<&str>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let date_played = date_played.map(parse_date_played).transpose()?;
    let update = state
        .playstate
        .mark_played_for_authorized_user(target_user_id, item_id, date_played)
        .await?;
    Ok(Json(update.into()))
}

pub(crate) async fn mark_unplayed(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_unplayed_for(state, &uri, headers, Some(user_id), item_id).await
}

async fn mark_unplayed_for(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let update = state
        .playstate
        .mark_unplayed_for_authorized_user(target_user_id, item_id)
        .await?;
    Ok(Json(update.into()))
}

impl From<PlaystateUpdate> for UserItemDataDto {
    fn from(update: PlaystateUpdate) -> Self {
        let data = update.user_data;
        Self {
            rating: data.rating,
            played_percentage: None,
            unplayed_item_count: None,
            playback_position_ticks: data.playback_position_ticks,
            play_count: data.play_count,
            is_favorite: data.is_favorite,
            likes: data.likes,
            last_played_date: data.last_played_date.map(format_date_played),
            played: data.played,
            key: data.custom_data_key,
            item_id: data.item_id.simple().to_string(),
        }
    }
}
