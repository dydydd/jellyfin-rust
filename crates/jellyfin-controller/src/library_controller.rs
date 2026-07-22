use jellyfin_data::{
    BaseItemError, BaseItemPage, BaseItemQuery, BaseItemRepository,
    entities::{base_item, user},
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

#[derive(Debug, Error)]
pub enum LibraryControllerError {
    #[error("target user was not found")]
    UserNotFound,
    #[error("library item was not found")]
    ItemNotFound,
    #[error("administrator access is required")]
    Forbidden,
    #[error("library item has no downloadable file")]
    FileNotFound,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

/// Coordinates `LibraryController` authorization with persisted item queries.
#[derive(Clone)]
pub struct LibraryControllerService {
    users: UserService,
    items: BaseItemRepository,
}

impl LibraryControllerService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database),
        }
    }

    /// Loads an item after validating access to the requested user context.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn item(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, LibraryControllerError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.items
            .get(item_id)
            .await?
            .ok_or(LibraryControllerError::ItemNotFound)
    }

    /// Returns persisted ancestors nearest-first.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn ancestors(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<Vec<base_item::Model>, LibraryControllerError> {
        self.item(authenticated_user, target_user_id, item_id)
            .await?;
        Ok(self
            .items
            .ancestors(item_id)
            .await?
            .into_iter()
            .map(|entry| entry.item)
            .collect())
    }

    /// Resolves the persisted file path for a downloadable item.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, missing-file, or persistence errors.
    pub async fn download_path(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<String, LibraryControllerError> {
        self.item(authenticated_user, target_user_id, item_id)
            .await?
            .path
            .filter(|path| !path.is_empty())
            .ok_or(LibraryControllerError::FileNotFound)
    }

    /// Finds persisted non-virtual items with the same item and media types.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn similar_items(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        let media_types = item.media_type.into_iter().collect();
        Ok(self
            .items
            .query(&BaseItemQuery {
                exclude_ids: vec![item.id],
                include_item_types: vec![item.item_type],
                media_types,
                is_virtual_item: Some(false),
                limit,
                ..Default::default()
            })
            .await?)
    }

    /// Atomically deletes complete item subtrees for an administrator.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-found, protected-item, or persistence errors.
    pub async fn delete_items(
        &self,
        authenticated_user: &user::Model,
        item_ids: &[Uuid],
    ) -> Result<(), LibraryControllerError> {
        if !authenticated_user.is_administrator {
            return Err(LibraryControllerError::Forbidden);
        }
        self.items.delete_many(item_ids).await?;
        Ok(())
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), LibraryControllerError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(LibraryControllerError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(LibraryControllerError::Forbidden);
        }
        Ok(())
    }
}
