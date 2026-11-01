use std::sync::Arc;

use axum::{
    Json,
    extract::{
        OriginalUri, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
};
use jellyfin_model::{TaskInfo, TaskTriggerInfo};
use serde::Deserialize;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ScheduledTasksQuery {
    #[serde(default, rename = "isHidden", alias = "IsHidden")]
    is_hidden: Option<bool>,
    #[serde(default, rename = "isEnabled", alias = "IsEnabled")]
    is_enabled: Option<bool>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<ScheduledTasksQuery>, QueryRejection>,
) -> Result<Json<Vec<TaskInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(
        state
            .scheduled_tasks
            .list(query.is_hidden, query.is_enabled)
            .await,
    ))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<TaskInfo>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.scheduled_tasks.get(&task_id).await?))
}

pub(crate) async fn start(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    state.scheduled_tasks.start(&task_id).await?;
    crate::websocket::broadcast_scheduled_tasks_info(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn stop(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    state.scheduled_tasks.stop(&task_id).await?;
    crate::websocket::broadcast_scheduled_tasks_info(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_triggers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    request: Result<Json<Vec<TaskTriggerInfo>>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Json(triggers) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .scheduled_tasks
        .update_triggers(&task_id, triggers)
        .await?;
    crate::websocket::broadcast_scheduled_tasks_info(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}
