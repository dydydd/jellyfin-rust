use std::sync::Arc;

use axum::{
    Json,
    extract::rejection::JsonRejection,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use jellyfin_model::{ExternalIdInfo, RemoteSearchResult};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ApplySearchCriteriaQuery {
    #[serde(default = "default_replace_all_images")]
    #[serde(rename = "replaceAllImages", alias = "ReplaceAllImages")]
    replace_all_images: bool,
}

pub(crate) async fn external_id_infos(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<ExternalIdInfo>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(state.item_lookup.external_id_infos(item_id).await?))
}

pub(crate) async fn remote_search(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Vec<RemoteSearchResult>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Json(_request) = request.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(state.item_lookup.remote_search()))
}

pub(crate) async fn remote_search_elevated(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Vec<RemoteSearchResult>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(_request) = request.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(state.item_lookup.remote_search()))
}

pub(crate) async fn apply_remote_search(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<ApplySearchCriteriaQuery>,
    request: Result<Json<RemoteSearchResult>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;
    let _replace_all_images = query.replace_all_images;
    let Json(result) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .item_lookup
        .apply_remote_search(item_id, result)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

const fn default_replace_all_images() -> bool {
    true
}
