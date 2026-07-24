use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use jellyfin_model::ImageInfo;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<ImageInfo>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let item = state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    Ok(Json(state.item_images.list(&item).await?))
}
