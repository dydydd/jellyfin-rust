use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::ItemUpdateInput;
use jellyfin_model::MetadataEditorInfo;
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

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpdateItemContentTypeQuery {
    #[serde(rename = "contentType", alias = "ContentType")]
    content_type: Option<String>,
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
    crate::websocket::broadcast_library_changed(
        &state,
        &[],
        &[],
        &[item_id.simple().to_string()],
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_content_type(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UpdateItemContentTypeQuery>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    state
        .item_update
        .update_content_type(item_id, query.content_type.as_deref())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn metadata_editor(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<MetadataEditorInfo>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(state.metadata_editor.get(item_id).await?))
}
