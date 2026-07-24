use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

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
