use jellyfin_data::{
    BaseItemError, BaseItemRepository, GenericUserDataPatch, PreferredUserDataKey, UserDataError,
    UserDataRepository,
    entities::{base_item, user_data},
};
use jellyfin_model::{UpdateUserItemDataDto, UserItemDataDto, UserPolicy};
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService, format_date_played};

#[derive(Debug, Error)]
pub enum UserDataServiceError {
    #[error("target user not found")]
    UserNotFound,
    #[error("item not found")]
    ItemNotFound,
    #[error("the authenticated identity cannot access user preferences")]
    Forbidden,
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
    pub runtime_ticks: Option<i64>,
}

impl From<UserDataUpdate> for UserItemDataDto {
    fn from(update: UserDataUpdate) -> Self {
        user_data_to_dto(update.user_data, update.runtime_ticks)
    }
}

/// Coordinates item visibility and atomic per-user data writes.
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
        let item = self.resolve_target_item(target_user_id, item_id).await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .set_favorite(item.id, target_user_id, &keys, is_favorite)
            .await?;
        Ok(UserDataUpdate {
            user_data,
            runtime_ticks: item.runtime_ticks,
        })
    }

    /// Sets or clears an item's boolean rating after target-user authorization.
    ///
    /// A nil item identifier addresses the persisted user root. `Some(true)`
    /// stores a like, `Some(false)` stores a dislike, and `None` clears both the
    /// numeric rating and boolean like value.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing user, item, or item hidden by the target
    /// user's folder policy, and returns persistence errors unchanged.
    pub async fn set_rating_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        likes: Option<bool>,
    ) -> Result<UserDataUpdate, UserDataServiceError> {
        let item = self.resolve_target_item(target_user_id, item_id).await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .set_rating(item.id, target_user_id, &keys, likes)
            .await?;
        Ok(UserDataUpdate {
            user_data,
            runtime_ticks: item.runtime_ticks,
        })
    }

    /// Loads generic user data without creating a persistence row.
    ///
    /// # Errors
    ///
    /// Returns forbidden when ordinary users cannot access preferences, and
    /// not-found for missing users, nil items, missing items, or hidden items.
    pub async fn get_item_data_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        bypass_preference_gate: bool,
    ) -> Result<UserDataUpdate, UserDataServiceError> {
        let item = self
            .resolve_generic_target_item(target_user_id, item_id, bypass_preference_gate)
            .await?;
        let keys = current_user_data_keys(&item);
        let user_data = match self
            .user_data
            .resolve_preferred(item.id, target_user_id, &keys)
            .await?
        {
            Some(data) => data,
            None => default_user_data(item.id, target_user_id, &keys[0]),
        };
        Ok(UserDataUpdate {
            user_data,
            runtime_ticks: item.runtime_ticks,
        })
    }

    /// Atomically applies a generic user-data update.
    ///
    /// Compatibility-only DTO fields (`PlayedPercentage`,
    /// `UnplayedItemCount`, `Key`, and `ItemId`) are intentionally ignored.
    ///
    /// # Errors
    ///
    /// Returns forbidden or not-found after authorization and visibility
    /// checks, and returns numeric validation and persistence errors unchanged.
    pub async fn update_item_data_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        bypass_preference_gate: bool,
        update: UpdateUserItemDataDto,
    ) -> Result<UserDataUpdate, UserDataServiceError> {
        let item = self
            .resolve_generic_target_item(target_user_id, item_id, bypass_preference_gate)
            .await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .apply_generic_patch(
                item.id,
                target_user_id,
                &keys,
                GenericUserDataPatch {
                    rating: update.rating,
                    playback_position_ticks: update.playback_position_ticks,
                    play_count: update.play_count,
                    is_favorite: update.is_favorite,
                    likes: update.likes,
                    last_played_date: update.last_played_date,
                    played: update.played,
                },
            )
            .await?;
        Ok(UserDataUpdate {
            user_data,
            runtime_ticks: item.runtime_ticks,
        })
    }

    /// Resolves preferred user-data rows for already-authorized items.
    ///
    /// This method deliberately does not repeat visibility checks; callers use
    /// it after obtaining an item page from a user-scoped library query.
    ///
    /// # Errors
    ///
    /// Returns persistence errors unchanged.
    pub async fn get_preferred_for_items(
        &self,
        target_user_id: Uuid,
        items: &[base_item::Model],
    ) -> Result<HashMap<Uuid, user_data::Model>, UserDataServiceError> {
        let keys = items
            .iter()
            .flat_map(|item| {
                current_user_data_keys(item)
                    .into_iter()
                    .enumerate()
                    .map(|(index, key)| {
                        PreferredUserDataKey::new(
                            item.id,
                            key,
                            i32::try_from(index + 1).unwrap_or(i32::MAX),
                        )
                    })
            })
            .collect::<Vec<_>>();
        Ok(self
            .user_data
            .resolve_preferred_for_items(target_user_id, &keys)
            .await?)
    }

    async fn resolve_generic_target_item(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        bypass_preference_gate: bool,
    ) -> Result<base_item::Model, UserDataServiceError> {
        let user = match self.users.get(target_user_id).await {
            Ok(user) => user,
            Err(UserError::NotFound) => return Err(UserDataServiceError::UserNotFound),
            Err(error) => return Err(error.into()),
        };
        let policy: UserPolicy = serde_json::from_value(user.policy.clone())
            .map_err(UserDataServiceError::InvalidPolicy)?;
        if !bypass_preference_gate && !policy.enable_user_preference_access {
            return Err(UserDataServiceError::Forbidden);
        }
        if item_id.is_nil() {
            return Err(UserDataServiceError::ItemNotFound);
        }
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(UserDataServiceError::ItemNotFound)?;
        if !self.is_visible_with_policy(&item, &policy).await? {
            return Err(UserDataServiceError::ItemNotFound);
        }
        Ok(item)
    }

    async fn resolve_target_item(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, UserDataServiceError> {
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
        Ok(item)
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
        self.is_visible_with_policy(item, &policy).await
    }

    async fn is_visible_with_policy(
        &self,
        item: &base_item::Model,
        policy: &UserPolicy,
    ) -> Result<bool, UserDataServiceError> {
        if item.item_type == "UserRootFolder" {
            return Ok(true);
        }
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

pub(crate) fn current_user_data_keys(item: &base_item::Model) -> Vec<String> {
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

fn default_user_data(item_id: Uuid, user_id: Uuid, key: &str) -> user_data::Model {
    user_data::Model {
        item_id,
        user_id,
        custom_data_key: key.to_owned(),
        rating: None,
        playback_position_ticks: 0,
        play_count: 0,
        is_favorite: false,
        last_played_date: None,
        played: false,
        audio_stream_index: None,
        subtitle_stream_index: None,
        likes: None,
        retention_date: None,
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Jellyfin's public played percentage is a floating-point ratio of 64-bit ticks"
)]
pub(crate) fn user_data_to_dto(
    data: user_data::Model,
    runtime_ticks: Option<i64>,
) -> UserItemDataDto {
    let played_percentage = runtime_ticks
        .filter(|runtime_ticks| *runtime_ticks > 0 && data.playback_position_ticks > 0)
        .map(|runtime_ticks| 100.0 * data.playback_position_ticks as f64 / runtime_ticks as f64);
    UserItemDataDto {
        rating: data.rating,
        played_percentage,
        unplayed_item_count: None,
        playback_position_ticks: data.playback_position_ticks,
        play_count: data.play_count,
        is_favorite: data.is_favorite,
        likes: data.rating.map(|rating| rating >= 6.5),
        last_played_date: data.last_played_date.map(format_date_played),
        played: data.played,
        key: data.custom_data_key,
        item_id: data.item_id.simple().to_string(),
    }
}
