use jellyfin_data::{BaseItemError, BaseItemRepository};
use thiserror::Error;
use uuid::Uuid;

use crate::ItemTypeRegistry;

#[derive(Debug, Error)]
pub enum VideoError {
    #[error("video was not found")]
    NotFound,
    #[error("administrator access is required")]
    Forbidden,
    #[error("at least two videos are required")]
    NotEnoughVideos,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

/// Coordinates administrator-only video version mutations.
#[derive(Clone)]
pub struct VideoService {
    items: BaseItemRepository,
    item_types: ItemTypeRegistry,
}

impl VideoService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self::with_item_type_registry(database, ItemTypeRegistry::default())
    }

    #[must_use]
    pub fn with_item_type_registry(
        database: impl Into<jellyfin_data::SharedDatabase>,
        item_types: ItemTypeRegistry,
    ) -> Self {
        Self {
            items: BaseItemRepository::new(database),
            item_types,
        }
    }

    /// Detaches all alternate sources in the requested video's version group.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-found, or persistence errors.
    pub async fn clear_alternate_sources(
        &self,
        is_administrator: bool,
        item_id: Uuid,
    ) -> Result<(), VideoError> {
        if !is_administrator {
            return Err(VideoError::Forbidden);
        }
        let item = self.items.get(item_id).await?.ok_or(VideoError::NotFound)?;
        let item_type = self
            .item_types
            .resolve(&item.item_type)
            .ok_or(VideoError::NotFound)?;
        if !matches!(
            item_type.name(),
            "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
        ) {
            return Err(VideoError::NotFound);
        }
        self.items.clear_alternate_sources(item_id).await?;
        Ok(())
    }

    /// Merges two or more videos into one version group.
    ///
    /// Missing and non-video ids are ignored before applying Jellyfin's
    /// "at least two videos" validation.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-enough-videos, or persistence errors.
    pub async fn merge_versions(
        &self,
        is_administrator: bool,
        item_ids: &[Uuid],
    ) -> Result<Uuid, VideoError> {
        if !is_administrator {
            return Err(VideoError::Forbidden);
        }

        let mut video_ids = Vec::new();
        for item in self.items.get_many(item_ids).await? {
            let Some(item_type) = self.item_types.resolve(&item.item_type) else {
                continue;
            };
            if matches!(
                item_type.name(),
                "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
            ) && !video_ids.contains(&item.id)
            {
                video_ids.push(item.id);
            }
        }

        if video_ids.len() < 2 {
            return Err(VideoError::NotEnoughVideos);
        }

        self.items
            .merge_linked_alternate_versions(&video_ids)
            .await
            .map_err(VideoError::BaseItem)
    }
}
