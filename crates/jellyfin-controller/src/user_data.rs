use jellyfin_data::{
    BaseItemError, BaseItemRepository, UserDataError, UserDataRepository,
    entities::{base_item, user_data},
};
use jellyfin_model::{UserItemDataDto, UserPolicy};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService, format_date_played};

#[derive(Debug, Error)]
pub enum UserDataServiceError {
    #[error("target user not found")]
    UserNotFound,
    #[error("item not found")]
    ItemNotFound,
    #[error("stored user policy is invalid")]
    InvalidPolicy(#[source] serde_json::Error),
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    UserData(#[from] UserDataError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserDataUpdate {
    pub user_data: user_data::Model,
}

impl From<UserDataUpdate> for UserItemDataDto {
    fn from(update: UserDataUpdate) -> Self {
        user_data_to_dto(update.user_data)
    }
}

/// Coordinates item visibility and atomic per-user favorite writes.
#[derive(Clone)]
pub struct UserDataService {
    users: UserService,
    items: BaseItemRepository,
    user_data: UserDataRepository,
}

impl UserDataService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            user_data: UserDataRepository::new(database),
        }
    }

    /// Sets an item's favorite state after the API layer authorizes the target user.
    ///
    /// A nil item identifier addresses the persisted user root, matching the
    /// official favorite endpoints.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing user, item, or item hidden by the target
    /// user's folder policy, and returns persistence errors unchanged.
    pub async fn set_favorite_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        is_favorite: bool,
    ) -> Result<UserDataUpdate, UserDataServiceError> {
        let user = match self.users.get(target_user_id).await {
            Ok(user) => user,
            Err(UserError::NotFound) => return Err(UserDataServiceError::UserNotFound),
            Err(error) => return Err(error.into()),
        };
        let item = if item_id.is_nil() {
            self.items.ensure_user_root().await?
        } else {
            self.items
                .get(item_id)
                .await?
                .ok_or(UserDataServiceError::ItemNotFound)?
        };
        if !self.is_visible(&item, &user.policy).await? {
            return Err(UserDataServiceError::ItemNotFound);
        }
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .set_favorite(item.id, target_user_id, &keys, is_favorite)
            .await?;
        Ok(UserDataUpdate { user_data })
    }

    async fn is_visible(
        &self,
        item: &base_item::Model,
        stored_policy: &serde_json::Value,
    ) -> Result<bool, UserDataServiceError> {
        if item.item_type == "UserRootFolder" {
            return Ok(true);
        }
        let policy: UserPolicy = serde_json::from_value(stored_policy.clone())
            .map_err(UserDataServiceError::InvalidPolicy)?;
        let ancestors = self.items.ancestors(item.id).await?;
        let collection_folders = std::iter::once(item)
            .chain(ancestors.iter().map(|entry| &entry.item))
            .filter(|candidate| candidate.item_type == "CollectionFolder")
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        if collection_folders.is_empty() {
            return Ok(true);
        }
        if let Some(blocked) = policy
            .blocked_media_folders
            .as_ref()
            .filter(|blocked| !blocked.is_empty())
        {
            return Ok(!collection_folders.iter().any(|id| blocked.contains(id)));
        }
        Ok(policy.enable_all_folders
            || collection_folders
                .iter()
                .any(|id| policy.enabled_folders.contains(id)))
    }
}

fn current_user_data_keys(item: &base_item::Model) -> Vec<String> {
    let id = item.id.to_string();
    match item
        .presentation_unique_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty() && *key != id)
    {
        Some(key) => vec![key.to_owned(), id],
        None => vec![id],
    }
}

pub(crate) fn user_data_to_dto(data: user_data::Model) -> UserItemDataDto {
    UserItemDataDto {
        rating: data.rating,
        played_percentage: None,
        unplayed_item_count: None,
        playback_position_ticks: data.playback_position_ticks,
        play_count: data.play_count,
        is_favorite: data.is_favorite,
        likes: data.likes,
        last_played_date: data.last_played_date.map(format_date_played),
        played: data.played,
        key: data.custom_data_key,
        item_id: data.item_id.simple().to_string(),
    }
}
