use std::sync::Arc;

use axum::{
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::{BaseItemError, BaseItemRepository};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
enum MetadataRefreshMode {
    #[default]
    None,
    ValidationOnly,
    Default,
    FullRefresh,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RefreshItemQuery {
    #[serde(rename = "metadataRefreshMode", alias = "MetadataRefreshMode")]
    metadata_refresh_mode: MetadataRefreshMode,
    #[serde(rename = "imageRefreshMode", alias = "ImageRefreshMode")]
    image_refresh_mode: MetadataRefreshMode,
    #[serde(rename = "replaceAllMetadata", alias = "ReplaceAllMetadata")]
    replace_all_metadata: bool,
    #[serde(rename = "replaceAllImages", alias = "ReplaceAllImages")]
    replace_all_images: bool,
    #[serde(rename = "regenerateTrickplay", alias = "RegenerateTrickplay")]
    regenerate_trickplay: bool,
}

pub(crate) async fn refresh(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<RefreshItemQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let _force_save = query.metadata_refresh_mode == MetadataRefreshMode::FullRefresh
        || query.image_refresh_mode == MetadataRefreshMode::FullRefresh
        || query.replace_all_images
        || query.replace_all_metadata;
    let _regenerate_trickplay = query.regenerate_trickplay;

    BaseItemRepository::new(state.database.clone())
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;

    Ok(StatusCode::NO_CONTENT)
}
