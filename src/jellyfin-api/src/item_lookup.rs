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

use crate::{ApiError, AppState, authentication, authorization};

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
    apply_configured_locale(&state, &mut request).await?;
    let kind = remote_search_kind(&uri);
    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let metadata_options = metadata_options_for(&state, kind);
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
    apply_configured_locale(&state, &mut request).await?;
    let kind = remote_search_kind(&uri);
    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let metadata_options = metadata_options_for(&state, kind);
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

fn metadata_options_for(_state: &AppState, kind: &str) -> jellyfin_model::MetadataOptions {
    jellyfin_model::MetadataOptions::official_defaults()
        .into_iter()
        .find(|options| options.item_type.eq_ignore_ascii_case(kind))
        .unwrap_or_default()
}

async fn apply_configured_locale(
    state: &AppState,
    request: &mut RemoteSearchRequest,
) -> Result<(), ApiError> {
    if request.search_info.metadata_language.is_some()
        && request.search_info.metadata_country_code.is_some()
    {
        return Ok(());
    }
    let configuration = state.server_configuration.load().await?;
    request
        .search_info
        .metadata_language
        .get_or_insert(configuration.preferred_metadata_language);
    request
        .search_info
        .metadata_country_code
        .get_or_insert(configuration.metadata_country_code);
    Ok(())
}

pub(crate) async fn apply_remote_search(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    request: Result<Json<RemoteSearchResult>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;
    let Json(result) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .item_lookup
        .apply_remote_search(item_id, result)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
