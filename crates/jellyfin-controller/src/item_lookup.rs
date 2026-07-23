use jellyfin_data::{BaseItemError, BaseItemRepository};
use jellyfin_model::{ExternalIdInfo, RemoteSearchResult};
use sea_orm::DatabaseConnection;
use serde_json::Value;
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

    /// Searches metadata providers for remote matches.
    ///
    /// No metadata providers are wired yet, so authenticated requests return
    /// Jellyfin's empty result shape.
    #[must_use]
    pub fn remote_search(&self) -> Vec<RemoteSearchResult> {
        Vec::new()
    }

    /// Applies remote-search provider identifiers to a persisted item.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item or persistence errors from
    /// `PostgreSQL`.
    pub async fn apply_remote_search(
        &self,
        item_id: Uuid,
        result: RemoteSearchResult,
    ) -> Result<(), ItemLookupError> {
        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        if !matches!(item.data, Some(Value::Object(_))) {
            item.data = Some(Value::Object(Default::default()));
        }
        if let Some(Value::Object(metadata)) = item.data.as_mut() {
            metadata.insert(
                "ProviderIds".to_owned(),
                serde_json::to_value(result.provider_ids)
                    .unwrap_or_else(|_| Value::Object(Default::default())),
            );
            metadata.remove("provider_ids");
        }
        self.items.update(item).await?;
        Ok(())
    }
}
