use jellyfin_data::{BaseItemError, BaseItemRepository};
use jellyfin_model::ExternalIdInfo;
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ItemLookupError {
    #[error("item was not found")]
    NotFound,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

/// Resolves persisted items against the registered metadata providers.
#[derive(Clone)]
pub struct ItemLookupService {
    items: BaseItemRepository,
}

impl ItemLookupService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            items: BaseItemRepository::new(database),
        }
    }

    /// Returns every registered external identifier supported by an item.
    ///
    /// # Errors
    ///
    /// Returns [`ItemLookupError::NotFound`] for an unknown item or the
    /// corresponding `PostgreSQL` persistence error.
    pub async fn external_id_infos(
        &self,
        item_id: Uuid,
    ) -> Result<Vec<ExternalIdInfo>, ItemLookupError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        Ok(jellyfin_providers::external_id::external_id_infos(
            &item.item_type,
        ))
    }
}
