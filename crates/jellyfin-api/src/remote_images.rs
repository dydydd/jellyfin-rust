use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use jellyfin_data::{BaseItemError, BaseItemRepository};
use jellyfin_model::{ImageProviderInfo, ImageType, RemoteImageResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RemoteImagesQuery {
    #[serde(default, rename = "type", alias = "Type")]
    image_type: Option<ImageType>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: Option<usize>,
    #[serde(default, rename = "limit", alias = "Limit")]
    limit: Option<usize>,
    #[serde(default, rename = "providerName", alias = "ProviderName")]
    provider_name: Option<String>,
    #[serde(default, rename = "includeAllLanguages", alias = "IncludeAllLanguages")]
    include_all_languages: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DownloadRemoteImageQuery {
    #[serde(default, rename = "type", alias = "Type")]
    image_type: Option<ImageType>,
    #[serde(default, rename = "imageUrl", alias = "ImageUrl")]
    image_url: Option<String>,
}

pub(crate) async fn images(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<RemoteImagesQuery>,
) -> Result<Json<RemoteImageResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;

    // Remote image providers are not wired yet. The query is still parsed so
    // clients can rely on the official parameter surface while receiving the
    // provider aggregate's empty result.
    let RemoteImagesQuery {
        image_type: _image_type,
        start_index: _start_index,
        limit: _limit,
        provider_name: _provider_name,
        include_all_languages: _include_all_languages,
    } = query;
    Ok(Json(RemoteImageResult {
        images: Vec::new(),
        total_record_count: 0,
        providers: Vec::new(),
    }))
}

pub(crate) async fn providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<ImageProviderInfo>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    Ok(Json(Vec::new()))
}

pub(crate) async fn download(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<DownloadRemoteImageQuery>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;
    let _image_type = query.image_type.ok_or(ApiError::InvalidRequest)?;
    let _image_url = query.image_url;
    ensure_item_exists(&state, item_id).await?;

    // No remote image providers are available yet, so there is nothing to
    // download after the official authorization and item validation gates.
    Err(BaseItemError::NotFound.into())
}

async fn ensure_item_exists(state: &AppState, item_id: Uuid) -> Result<(), ApiError> {
    BaseItemRepository::new(state.database.clone())
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;
    Ok(())
}
