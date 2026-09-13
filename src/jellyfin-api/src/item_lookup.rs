use std::sync::Arc;

use axum::{
    Json,
    extract::rejection::JsonRejection,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::{RemoteSearchInfo, RemoteSearchRequest};
use jellyfin_model::{ExternalIdInfo, RemoteSearchResult};
use serde::Deserialize;
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

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct EmbyGameRemoteSearchRequest {
    #[serde(alias = "searchInfo", alias = "searchinfo")]
    search_info: Option<RemoteSearchInfo>,
    #[serde(
        alias = "itemId",
        alias = "itemid",
        deserialize_with = "deserialize_optional_i64"
    )]
    _item_id: Option<i64>,
    #[serde(alias = "searchProviderName", alias = "searchprovidername")]
    search_provider_name: Option<String>,
    #[serde(alias = "providers")]
    _providers: Vec<String>,
    #[serde(alias = "includeDisabledProviders", alias = "includedisabledproviders")]
    include_disabled_providers: Option<bool>,
}

pub(crate) async fn emby_game_remote_search(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<EmbyGameRemoteSearchRequest>, JsonRejection>,
) -> Result<Json<Vec<RemoteSearchResult>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let mut request = RemoteSearchRequest {
        search_info: request.search_info.unwrap_or_default(),
        item_id: None,
        search_provider_name: request.search_provider_name,
        include_disabled_providers: request.include_disabled_providers.unwrap_or_default(),
    };
    let configuration = state.server_configuration.load().await?;
    apply_configured_locale(&configuration, &mut request);
    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let metadata_options = crate::configuration::metadata_options(&configuration)?;
    Ok(Json(
        state
            .item_lookup
            .remote_search("Game", request, &api_key, &metadata_options)
            .await?,
    ))
}

fn deserialize_optional_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Integer {
        Number(i64),
        String(String),
    }

    Option::<Integer>::deserialize(deserializer)?.map_or(Ok(None), |value| match value {
        Integer::Number(value) => Ok(Some(value)),
        Integer::String(value) => value.parse().map(Some).map_err(serde::de::Error::custom),
    })
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
