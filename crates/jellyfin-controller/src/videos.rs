use jellyfin_data::{BaseItemError, BaseItemRepository, entities::user};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::ItemTypeRegistry;

#[derive(Debug, Error)]
pub enum VideoError {
    #[error("video was not found")]
    NotFound,
    #[error("administrator access is required")]
    Forbidden,
    #[error("item is not a video")]
    InvalidItemType,
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
    pub fn new(database: DatabaseConnection) -> Self {
        Self::with_item_type_registry(database, ItemTypeRegistry::default())
    }

    #[must_use]
    pub fn with_item_type_registry(
        database: DatabaseConnection,
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
    /// Returns forbidden, not-found, invalid-item-type, or persistence errors.
    pub async fn clear_alternate_sources(
        &self,
        authenticated_user: &user::Model,
        item_id: Uuid,
    ) -> Result<(), VideoError> {
        if !authenticated_user.is_administrator {
            return Err(VideoError::Forbidden);
        }
        let item = self.items.get(item_id).await?.ok_or(VideoError::NotFound)?;
        let item_type = self
            .item_types
            .resolve(&item.item_type)
            .ok_or(VideoError::InvalidItemType)?;
        if !matches!(item_type.name(), "Video" | "Movie") {
            return Err(VideoError::InvalidItemType);
        }
        self.items.clear_alternate_sources(item_id).await?;
        Ok(())
    }
}
