use std::sync::Arc;

use axum::{
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemUpdateRepository, ItemValueRepository, PersonRepository,
};
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
    /// Deprecated by Jellyfin clients; accepted for wire compatibility and ignored.
    #[serde(rename = "imageRefreshMode", alias = "ImageRefreshMode")]
    image_refresh_mode: MetadataRefreshMode,
    /// Accepted for wire compatibility and ignored by this refresh pipeline.
    #[serde(rename = "replaceAllMetadata", alias = "ReplaceAllMetadata")]
    replace_all_metadata: bool,
    /// Accepted for wire compatibility and ignored by this refresh pipeline.
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
    let item = BaseItemRepository::new(state.database.clone())
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;
    crate::websocket::broadcast_refresh_progress(&state, item_id, 0.0).await;

    if is_video_item(&item) {
        if query.regenerate_trickplay
            && query.metadata_refresh_mode == MetadataRefreshMode::FullRefresh
        {
            state.trickplay.delete_data(item_id).await?;
        } else if matches!(
            query.metadata_refresh_mode,
            MetadataRefreshMode::Default | MetadataRefreshMode::FullRefresh
        ) {
            let configuration = state.server_configuration.load().await?;
            let trickplay_options = serde_json::from_value::<jellyfin_model::TrickplayOptions>(
                configuration.trickplay_options,
            )
            .map_err(|_| ApiError::Internal)?;
            state
                .trickplay
                .discover_data(item_id, item.runtime_ticks, trickplay_options.interval)
                .await?;
        }
    }

    if item.item_type == "CollectionFolder"
        && query.metadata_refresh_mode != MetadataRefreshMode::None
    {
        crate::websocket::broadcast_refresh_progress(&state, item_id, 30.0).await;
        state.library_scan.scan_collection(item_id).await?;
        crate::websocket::broadcast_refresh_progress(&state, item_id, 80.0).await;
    }

    if matches!(
        query.metadata_refresh_mode,
        MetadataRefreshMode::Default | MetadataRefreshMode::FullRefresh
    ) && matches!(item.item_type.as_str(), "Movie" | "Series")
    {
        crate::websocket::broadcast_refresh_progress(&state, item_id, 40.0).await;
        let api_key = state.tmdb_api_key.read().await;
        if !api_key.is_empty() {
            let provider = jellyfin_controller::metadata_providers::TmdbMetadataProvider::new(
                api_key.clone(),
                BaseItemRepository::new(state.database.clone()),
                ItemValueRepository::new(state.database.clone()),
                PersonRepository::new(state.database.clone()),
                ItemUpdateRepository::new(state.database.clone()),
                Some(state.item_images.clone()),
            );
            drop(api_key);
            if let Err(error) = provider.refresh_item(item_id).await {
                eprintln!("TMDB metadata refresh failed: {error}");
            }
        }

        let omdb_api_key = state.omdb_api_key.read().await;
        if !omdb_api_key.is_empty() {
            let provider =
                jellyfin_controller::metadata_providers::OmdbMetadataProvider::new(
                    omdb_api_key.clone(),
                    BaseItemRepository::new(state.database.clone()),
                    ItemValueRepository::new(state.database.clone()),
                    ItemUpdateRepository::new(state.database.clone()),
                );
            drop(omdb_api_key);
            if let Err(error) = provider.refresh_item(item_id).await {
                eprintln!("OMDb metadata refresh failed: {error}");
            }
        }

        if matches!(item.item_type.as_str(), "MusicArtist" | "MusicAlbum") {
            let provider = jellyfin_controller::metadata_providers::AudioDbMetadataProvider::new(
                BaseItemRepository::new(state.database.clone()),
                ItemUpdateRepository::new(state.database.clone()),
            );
            if let Err(error) = provider.refresh_item(item_id).await {
                eprintln!("TheAudioDB metadata refresh failed: {error}");
            }
        }
        crate::websocket::broadcast_refresh_progress(&state, item_id, 90.0).await;
    }

    crate::websocket::broadcast_refresh_progress(&state, item_id, 100.0).await;
    Ok(StatusCode::NO_CONTENT)
}

fn is_video_item(item: &jellyfin_data::entities::base_item::Model) -> bool {
    item.media_type
        .as_deref()
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case("Video"))
        || matches!(
            item.item_type.as_str(),
            "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
        )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn refresh_query_accepts_official_deprecated_and_ignored_parameters() {
        let query: RefreshItemQuery = serde_json::from_value(json!({
            "MetadataRefreshMode": "Default",
            "ImageRefreshMode": "FullRefresh",
            "ReplaceAllMetadata": true,
            "ReplaceAllImages": true,
            "RegenerateTrickplay": false
        }))
        .expect("official refresh query parameters must parse");

        assert_eq!(query.metadata_refresh_mode, MetadataRefreshMode::Default);
        assert_eq!(query.image_refresh_mode, MetadataRefreshMode::FullRefresh);
        assert!(query.replace_all_metadata);
        assert!(query.replace_all_images);
        assert!(!query.regenerate_trickplay);
    }
}
