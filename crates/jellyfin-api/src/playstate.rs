use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::{
    PlaybackProgressUpdate, PlaybackStartUpdate, PlaybackStopUpdate, parse_date_played,
};
use jellyfin_model::UserItemDataDto;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    authentication::{AuthenticatedIdentity, AuthenticatedSession},
    authorization,
};

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

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackProgressInfo {
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
    pub position_ticks: Option<i64>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackStartInfo {
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackStopInfo {
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
    pub position_ticks: Option<i64>,
    pub failed: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackStartQuery {
    #[serde(default, rename = "mediaSourceId", alias = "MediaSourceId")]
    pub media_source_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackProgressQuery {
    #[serde(default, rename = "mediaSourceId", alias = "MediaSourceId")]
    pub media_source_id: Option<String>,
    #[serde(default, rename = "positionTicks", alias = "PositionTicks")]
    pub position_ticks: Option<i64>,
    #[serde(default, rename = "audioStreamIndex", alias = "AudioStreamIndex")]
    pub audio_stream_index: Option<i32>,
    #[serde(default, rename = "subtitleStreamIndex", alias = "SubtitleStreamIndex")]
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackStopQuery {
    #[serde(default, rename = "mediaSourceId", alias = "MediaSourceId")]
    pub media_source_id: Option<String>,
    #[serde(default, rename = "positionTicks", alias = "PositionTicks")]
    pub position_ticks: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackPingQuery {
    #[serde(default, rename = "playSessionId", alias = "PlaySessionId")]
    pub play_session_id: Option<String>,
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

pub(crate) async fn report_playback_progress(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<PlaybackProgressInfo>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(progress) = request.map_err(|_| ApiError::InvalidRequest)?;
    report_playback_progress_for_current_session(state, &uri, headers, progress.into()).await
}

pub(crate) async fn report_playback_start(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<PlaybackStartInfo>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(start) = request.map_err(|_| ApiError::InvalidRequest)?;
    report_playback_start_for_current_session(state, &uri, headers, start.into()).await
}

pub(crate) async fn report_playback_stopped(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<PlaybackStopInfo>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(stop) = request.map_err(|_| ApiError::InvalidRequest)?;
    report_playback_stop_for_current_session(state, &uri, headers, stop.into()).await
}

pub(crate) async fn ping_playback_session(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PlaybackPingQuery>,
) -> Result<StatusCode, ApiError> {
    if query.play_session_id.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::InvalidRequest);
    }
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn report_playback_start_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlaybackStartQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_start_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStartUpdate {
            item_id,
            media_source_id: query.media_source_id,
        },
    )
    .await
}

pub(crate) async fn report_playback_stopped_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlaybackStopQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_stop_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStopUpdate {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            failed: false,
        },
    )
    .await
}

pub(crate) async fn report_playback_start_legacy_for_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<PlaybackStartQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_start_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStartUpdate {
            item_id,
            media_source_id: query.media_source_id,
        },
    )
    .await
}

pub(crate) async fn report_playback_stopped_legacy_for_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<PlaybackStopQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_stop_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStopUpdate {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            failed: false,
        },
    )
    .await
}

pub(crate) async fn report_playback_progress_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlaybackProgressQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_progress_for_current_session(
        state,
        &uri,
        headers,
        PlaybackProgressUpdate {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
        },
    )
    .await
}

pub(crate) async fn report_playback_progress_legacy_for_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<PlaybackProgressQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_progress_for_current_session(
        state,
        &uri,
        headers,
        PlaybackProgressUpdate {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
        },
    )
    .await
}

async fn report_playback_progress_for_current_session(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    update: PlaybackProgressUpdate,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    if let AuthenticatedIdentity::Device(session) = identity {
        record_device_playback_progress(&state, &session, update).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn report_playback_start_for_current_session(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    update: PlaybackStartUpdate,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    if let AuthenticatedIdentity::Device(session) = identity {
        record_device_playback_start(&state, &session, update).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn report_playback_stop_for_current_session(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    update: PlaybackStopUpdate,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    if let AuthenticatedIdentity::Device(session) = identity {
        record_device_playback_stop(&state, &session, update).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn record_device_playback_progress(
    state: &AppState,
    session: &AuthenticatedSession,
    update: PlaybackProgressUpdate,
) -> Result<(), ApiError> {
    state
        .playstate
        .report_playback_progress(&session.user, update)
        .await?;
    Ok(())
}

async fn record_device_playback_start(
    state: &AppState,
    session: &AuthenticatedSession,
    update: PlaybackStartUpdate,
) -> Result<(), ApiError> {
    state
        .playstate
        .report_playback_start(&session.user, update)
        .await?;
    Ok(())
}

async fn record_device_playback_stop(
    state: &AppState,
    session: &AuthenticatedSession,
    update: PlaybackStopUpdate,
) -> Result<(), ApiError> {
    state
        .playstate
        .report_playback_stop(&session.user, update)
        .await?;
    Ok(())
}

impl From<PlaybackProgressInfo> for PlaybackProgressUpdate {
    fn from(info: PlaybackProgressInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id,
            position_ticks: info.position_ticks,
            audio_stream_index: info.audio_stream_index,
            subtitle_stream_index: info.subtitle_stream_index,
        }
    }
}

impl From<PlaybackStopInfo> for PlaybackStopUpdate {
    fn from(info: PlaybackStopInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id,
            position_ticks: info.position_ticks,
            failed: info.failed,
        }
    }
}

impl From<PlaybackStartInfo> for PlaybackStartUpdate {
    fn from(info: PlaybackStartInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id,
        }
    }
}
