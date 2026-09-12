use std::sync::Arc;

use axum::{
    Json,
    extract::rejection::JsonRejection,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::RemoteSearchRequest;
use jellyfin_model::{ExternalIdInfo, RemoteSearchResult};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

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
    request: Result<Json<RemoteSearchRequest>, JsonRejection>,
) -> Result<Json<Vec<RemoteSearchResult>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Json(mut request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let configuration = state.server_configuration.load().await?;
    apply_configured_locale(&configuration, &mut request);
    let kind = remote_search_kind(&uri);
    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let metadata_options = crate::configuration::metadata_options(&configuration)?;
    Ok(Json(
        state
            .item_lookup
            .remote_search(kind, request, &api_key, &metadata_options)
            .await?,
    ))
}

pub(crate) async fn remote_search_elevated(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<RemoteSearchRequest>, JsonRejection>,
) -> Result<Json<Vec<RemoteSearchResult>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(mut request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let configuration = state.server_configuration.load().await?;
    apply_configured_locale(&configuration, &mut request);
    let kind = remote_search_kind(&uri);
    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let metadata_options = crate::configuration::metadata_options(&configuration)?;
    Ok(Json(
        state
            .item_lookup
            .remote_search(kind, request, &api_key, &metadata_options)
            .await?,
    ))
}

fn remote_search_kind(uri: &axum::http::Uri) -> &str {
    uri.path().rsplit('/').next().unwrap_or_default()
}

fn apply_configured_locale(
    configuration: &jellyfin_data::entities::server_configuration::Model,
    request: &mut RemoteSearchRequest,
) {
    if request.search_info.metadata_language.is_some()
        && request.search_info.metadata_country_code.is_some()
    {
        return;
    }
    request
        .search_info
        .metadata_language
        .get_or_insert_with(|| configuration.preferred_metadata_language.clone());
    request
        .search_info
        .metadata_country_code
        .get_or_insert_with(|| configuration.metadata_country_code.clone());
}

pub(crate) async fn apply_remote_search(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    request: Result<Json<RemoteSearchResult>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(result) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .item_lookup
        .apply_remote_search(item_id, result)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
