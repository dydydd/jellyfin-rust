use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_model::GroupInfoDto;
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
    Ok(Json(state.sync_play.list_groups().await))
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
    state
        .sync_play
        .get_group(group_id)
        .await
        .map(Json)
        .ok_or(ApiError::NotFound)
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
