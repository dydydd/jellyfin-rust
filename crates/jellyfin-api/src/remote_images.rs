use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use jellyfin_data::BaseItemError;
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

    let api_key = state.tmdb_api_key.read().await.clone();
    let result = state
        .item_lookup
        .remote_images(
            item_id,
            query.image_type,
            query.provider_name.as_deref(),
            query.include_all_languages,
            query.start_index.unwrap_or(0),
            query.limit,
            &api_key,
        )
        .await?;
    Ok(Json(result))
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
    let api_key = state.tmdb_api_key.read().await.clone();
    let metadata_options = metadata_options_for(&state);
    Ok(Json(
        state
            .item_lookup
            .remote_image_providers(item_id, &api_key, &metadata_options)
            .await?,
    ))
}

fn metadata_options_for(state: &AppState) -> jellyfin_model::MetadataOptions {
    let _ = state;
    jellyfin_model::MetadataOptions::official_defaults()
        .into_iter()
        .find(|options| options.item_type == "Movie")
        .unwrap_or_default()
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
    let image_type = query.image_type.ok_or(ApiError::InvalidRequest)?;
    let image_url = query.image_url.ok_or(ApiError::NotFound)?;
    ensure_item_exists(&state, item_id).await?;

    let api_key = state.tmdb_api_key.read().await.clone();
    if api_key.is_empty() {
        return Err(BaseItemError::NotFound.into());
    }
    state
        .item_images
        .download_remote_image(item_id, image_type, &image_url)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_item_exists(state: &AppState, item_id: Uuid) -> Result<(), ApiError> {
    state
        .base_items
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;
    Ok(())
}
