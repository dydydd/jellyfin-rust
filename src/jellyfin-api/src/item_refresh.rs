use std::sync::Arc;

use axum::{
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::{MetadataRefreshMode, MetadataRefreshOptions};
use jellyfin_data::BaseItemError;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

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
    let item = state
        .base_items
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
        queue_collection_refresh(state, item_id);
        return Ok(StatusCode::NO_CONTENT);
    }

    if query.metadata_refresh_mode != MetadataRefreshMode::None
        || query.image_refresh_mode != MetadataRefreshMode::None
    {
        crate::websocket::broadcast_refresh_progress(&state, item_id, 40.0).await;
        let tmdb_api_key = Arc::clone(&*state.tmdb_api_key.read().await);
        let omdb_api_key = Arc::clone(&*state.omdb_api_key.read().await);
        if let Err(error) = state
            .metadata_refresh
            .refresh(
                item_id,
                &tmdb_api_key,
                &omdb_api_key,
                MetadataRefreshOptions {
                    metadata_refresh_mode: query.metadata_refresh_mode,
                    image_refresh_mode: query.image_refresh_mode,
                    replace_all_metadata: query.replace_all_metadata,
                    replace_all_images: query.replace_all_images,
                },
            )
            .await
        {
            tracing::error!(%error, "metadata refresh failed");
        }
        crate::websocket::broadcast_refresh_progress(&state, item_id, 90.0).await;
    }

    crate::websocket::broadcast_refresh_progress(&state, item_id, 100.0).await;
    Ok(StatusCode::NO_CONTENT)
}

fn queue_collection_refresh(state: Arc<AppState>, item_id: Uuid) {
    tracing::info!(%item_id, "queued collection scan and missing metadata refresh");
    tokio::spawn(async move {
        crate::websocket::broadcast_refresh_progress(&state, item_id, 1.0).await;
        if let Err(error) = state.library_scan.scan_collection(item_id).await {
            tracing::error!(%error, %item_id, "collection scan failed");
            crate::websocket::broadcast_refresh_progress(&state, item_id, 100.0).await;
            return;
        }
        crate::websocket::broadcast_refresh_progress(&state, item_id, 60.0).await;

        let configured_concurrency = state
            .server_configuration
            .load()
            .await
            .map_or(0, |configuration| {
                configuration.library_metadata_refresh_concurrency
            });
        let concurrency = usize::try_from(configured_concurrency)
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or(2)
            .min(16);
        let tmdb_api_key = Arc::clone(&*state.tmdb_api_key.read().await);
        let omdb_api_key = Arc::clone(&*state.omdb_api_key.read().await);
        let progress_state = Arc::clone(&state);
        let (progress_sender, mut progress_receiver) = tokio::sync::watch::channel(60.0);
        let progress_reporter = tokio::spawn(async move {
            while progress_receiver.changed().await.is_ok() {
                let progress = *progress_receiver.borrow_and_update();
                crate::websocket::broadcast_refresh_progress(&progress_state, item_id, progress)
                    .await;
            }
        });
        let report_progress = move |progress: f64| {
            progress_sender.send_replace((60.0 + progress * 0.4).clamp(60.0, 100.0));
        };
        let refresh_result = state
            .metadata_refresh
            .refresh_missing_library_metadata(
                Some(item_id),
                &tmdb_api_key,
                &omdb_api_key,
                concurrency,
                &report_progress,
            )
            .await;
        drop(report_progress);
        if let Err(error) = progress_reporter.await {
            tracing::debug!(%error, %item_id, "collection refresh progress reporter failed");
        }
        match refresh_result {
            Ok(summary) => tracing::info!(
                %item_id,
                candidates = summary.candidates,
                refreshed = summary.refreshed,
                unchanged = summary.unchanged,
                failed = summary.failed,
                missing_episodes = summary.missing_episodes,
                "collection scan and missing metadata refresh completed"
            ),
            Err(error) => {
                tracing::error!(%error, %item_id, "collection metadata refresh failed");
            }
        }
        crate::websocket::broadcast_refresh_progress(&state, item_id, 100.0).await;
    });
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
