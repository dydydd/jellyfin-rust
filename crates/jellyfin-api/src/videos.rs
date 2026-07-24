use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MergeVersionsQuery {
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    ids: Vec<Uuid>,
}

pub(crate) async fn delete_alternate_sources(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .videos
        .clear_alternate_sources(&authenticated.user, item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn merge_versions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MergeVersionsQuery>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .videos
        .merge_versions(&authenticated.user, &query.ids)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn additional_parts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<user_library::UserIdQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let items = state
        .user_library
        .additional_parts(&authenticated.user, target_user_id, item_id)
        .await?
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}
