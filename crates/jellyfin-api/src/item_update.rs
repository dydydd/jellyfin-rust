use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::ItemUpdateInput;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct UpdateItemRequest {
    tags: Option<Vec<String>>,
    genres: Option<Vec<String>>,
    provider_ids: Option<BTreeMap<String, Option<String>>>,
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    request: Result<Json<UpdateItemRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .item_update
        .update(
            item_id,
            ItemUpdateInput {
                tags: request.tags,
                genres: request.genres,
                provider_ids: request.provider_ids,
            },
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
