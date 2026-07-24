use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_model::{GroupInfoDto, GroupQueueMode};
use jellyfin_server_implementations::SyncPlaySession;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct NewGroupRequest {
    group_name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct JoinGroupRequest {
    group_id: Uuid,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct PlayRequest {
    playing_queue: Vec<Uuid>,
    playing_item_position: i32,
    start_position_ticks: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct SetPlaylistItemRequest {
    playlist_item_id: Uuid,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct RemoveFromPlaylistRequest {
    playlist_item_ids: Vec<Uuid>,
    clear_playlist: bool,
    clear_playing_item: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct MovePlaylistItemRequest {
    playlist_item_id: Uuid,
    new_index: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct QueueRequest {
    item_ids: Vec<Uuid>,
    mode: GroupQueueMode,
}

pub(crate) async fn create_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<NewGroupRequest>, JsonRejection>,
) -> Result<Json<GroupInfoDto>, ApiError> {
    let session = authentication::authenticated_session(&state, &headers).await?;
    if !session.can_create_sync_play_group() {
        return Err(ApiError::Forbidden);
    }
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let group_name = request.group_name.trim().to_owned();
    if group_name.chars().count() > 200 {
        return Err(ApiError::InvalidRequest);
    }

    Ok(Json(
        state
            .sync_play
            .create_group(sync_play_session(&session), group_name)
            .await,
    ))
}

pub(crate) async fn join_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<JoinGroupRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let session = authentication::authenticated_session(&state, &headers).await?;
    if !session.can_join_sync_play_group() {
        return Err(ApiError::Forbidden);
    }
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let item_ids = state
        .sync_play
        .queue_item_ids_for_group(request.group_id)
        .await
        .unwrap_or_default();
    if !user_can_access_items(&state, &session.user, &item_ids).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    state
        .sync_play
        .join_group(sync_play_session(&session), request.group_id)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn leave_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let session = authentication::authenticated_session(&state, &headers).await?;
    if !state.sync_play.is_user_active(session.user.id).await {
        return Err(ApiError::Forbidden);
    }
    state
        .sync_play
        .leave_group(&crate::session::jellyfin_session_id(
            &session.device.app_name,
            &session.device.device_id,
        ))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_groups(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<GroupInfoDto>>, ApiError> {
    let session = authentication::authenticated_session(&state, &headers).await?;
    if !session.can_join_sync_play_group() {
        return Err(ApiError::Forbidden);
    }
    let mut visible = Vec::new();
    for group in state.sync_play.list_groups().await {
        let item_ids = state
            .sync_play
            .queue_item_ids_for_group(group.group_id)
            .await
            .unwrap_or_default();
        if user_can_access_items(&state, &session.user, &item_ids).await? {
            visible.push(group);
        }
    }
    Ok(Json(visible))
}

pub(crate) async fn get_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(group_id): Path<Uuid>,
) -> Result<Json<GroupInfoDto>, ApiError> {
    let session = authentication::authenticated_session(&state, &headers).await?;
    if !session.can_join_sync_play_group() {
        return Err(ApiError::Forbidden);
    }
    let group = state
        .sync_play
        .get_group(group_id)
        .await
        .ok_or(ApiError::NotFound)?;
    let item_ids = state
        .sync_play
        .queue_item_ids_for_group(group_id)
        .await
        .unwrap_or_default();
    if !user_can_access_items(&state, &session.user, &item_ids).await? {
        return Err(ApiError::NotFound);
    }
    Ok(Json(group))
}

pub(crate) async fn set_new_queue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<PlayRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let session = require_active_user(&state, &headers).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    if queue_position_is_valid(&request.playing_queue, request.playing_item_position) {
        if group_can_access_items(&state, &session, &request.playing_queue).await? {
            state
                .sync_play
                .set_new_queue(
                    &session.session_id,
                    &request.playing_queue,
                    request.playing_item_position,
                    request.start_position_ticks,
                )
                .await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn set_playlist_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<SetPlaylistItemRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let session = require_active_user(&state, &headers).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .sync_play
        .set_playlist_item(&session.session_id, request.playlist_item_id)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_from_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<RemoveFromPlaylistRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let session = require_active_user(&state, &headers).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .sync_play
        .remove_from_playlist(
            &session.session_id,
            &request.playlist_item_ids,
            request.clear_playlist,
            request.clear_playing_item,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn move_playlist_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<MovePlaylistItemRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let session = require_active_user(&state, &headers).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .sync_play
        .move_playlist_item(
            &session.session_id,
            request.playlist_item_id,
            request.new_index,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn queue_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<QueueRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let session = require_active_user(&state, &headers).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    if !request.item_ids.is_empty() {
        if group_can_access_items(&state, &session, &request.item_ids).await? {
            state
                .sync_play
                .queue_items(&session.session_id, &request.item_ids, request.mode)
                .await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn require_active_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<SyncPlaySession, ApiError> {
    let authenticated = authentication::authenticated_session(state, headers).await?;
    if !state.sync_play.is_user_active(authenticated.user.id).await {
        return Err(ApiError::Forbidden);
    }
    Ok(sync_play_session(&authenticated))
}

async fn group_can_access_items(
    state: &AppState,
    session: &SyncPlaySession,
    item_ids: &[Uuid],
) -> Result<bool, ApiError> {
    let Some(user_ids) = state
        .sync_play
        .participant_user_ids_for_session(&session.session_id)
        .await
    else {
        return Ok(false);
    };
    for user_id in user_ids {
        let user = state.users.get(user_id).await?;
        if !user_can_access_items(state, &user, item_ids).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn user_can_access_items(
    state: &AppState,
    user: &jellyfin_data::entities::user::Model,
    item_ids: &[Uuid],
) -> Result<bool, ApiError> {
    for item_id in item_ids {
        match state.user_library.item(user, user.id, *item_id).await {
            Ok(_) => {}
            Err(
                jellyfin_controller::UserLibraryError::ItemNotFound
                | jellyfin_controller::UserLibraryError::UserNotFound
                | jellyfin_controller::UserLibraryError::Forbidden,
            ) => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

fn queue_position_is_valid(item_ids: &[Uuid], playing_item_position: i32) -> bool {
    usize::try_from(playing_item_position)
        .ok()
        .is_some_and(|position| !item_ids.is_empty() && position < item_ids.len())
}

fn sync_play_session(session: &authentication::AuthenticatedSession) -> SyncPlaySession {
    SyncPlaySession {
        session_id: crate::session::jellyfin_session_id(
            &session.device.app_name,
            &session.device.device_id,
        ),
        user_id: session.user.id,
        user_name: session.user.username.clone(),
    }
}
