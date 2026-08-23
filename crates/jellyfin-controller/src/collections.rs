use jellyfin_data::{
    BaseItemError, BaseItemRepository, CollectionRepository, CollectionStoreError,
    LinkedChildRepository, LinkedChildStoreError,
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("the supplied collection id is not a BoxSet")]
    InvalidCollection,
    #[error("the collection name cannot be blank")]
    InvalidName,
    #[error(transparent)]
    CollectionStore(#[from] CollectionStoreError),
    #[error(transparent)]
    LinkedChildStore(#[from] LinkedChildStoreError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

/// Coordinates `BoxSet` validation with ordered linked-child persistence.
#[derive(Clone)]
pub struct CollectionService {
    items: BaseItemRepository,
    collections: CollectionRepository,
    linked_children: LinkedChildRepository,
}

impl CollectionService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            items: BaseItemRepository::new(database.clone()),
            collections: CollectionRepository::new(database.clone()),
            linked_children: LinkedChildRepository::new(database),
        }
    }

    /// Creates a persisted `BoxSet` with its initial manual members atomically.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors.
    pub async fn create(
        &self,
        name: Option<String>,
        parent_id: Option<Uuid>,
        is_locked: bool,
        child_ids: &[Uuid],
    ) -> Result<Uuid, CollectionError> {
        let name = name.map(|name| name.trim().to_owned());
        if name.as_ref().is_some_and(String::is_empty) {
            return Err(CollectionError::InvalidName);
        }
        let parent_id = match parent_id {
            Some(parent_id) => Some(parent_id),
            None => Some(self.items.ensure_user_root().await?.id),
        };
        let id = Uuid::new_v4();
        self.collections
            .create(id, name, parent_id, is_locked, child_ids)
            .await?;
        Ok(id)
    }

    /// Appends manual members to an existing `BoxSet`.
    ///
    /// # Errors
    ///
    /// Returns invalid-collection, missing-child, or persistence errors.
    pub async fn add(&self, collection_id: Uuid, ids: &[Uuid]) -> Result<(), CollectionError> {
        self.require_box_set(collection_id).await?;
        self.linked_children.add_manual(collection_id, ids).await?;
        Ok(())
    }

    /// Removes matching manual members from an existing `BoxSet`.
    ///
    /// # Errors
    ///
    /// Returns invalid-collection or persistence errors. Missing member links are ignored.
    pub async fn remove(&self, collection_id: Uuid, ids: &[Uuid]) -> Result<(), CollectionError> {
        self.require_box_set(collection_id).await?;
        self.linked_children.remove(collection_id, ids).await?;
        Ok(())
    }

    async fn require_box_set(&self, collection_id: Uuid) -> Result<(), CollectionError> {
        if self
            .items
            .get(collection_id)
            .await?
            .is_some_and(|item| item.item_type == "BoxSet")
        {
            Ok(())
        } else {
            Err(CollectionError::InvalidCollection)
        }
    }
}
