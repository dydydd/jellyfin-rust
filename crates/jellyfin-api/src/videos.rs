use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

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
