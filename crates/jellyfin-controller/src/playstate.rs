use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, UserDataError, UserDataRepository,
    entities::{user, user_data},
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use jellyfin_model::UserItemDataDto;

use crate::{UserError, UserService, user_data::user_data_to_dto};

#[derive(Debug, Error)]
pub enum PlaystateError {
    #[error("target user not found")]
    UserNotFound,
    #[error("item not found")]
    ItemNotFound,
    #[error("the authenticated user cannot update this user's playstate")]
    Forbidden,
    #[error("invalid date played")]
    InvalidDatePlayed,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    UserData(#[from] UserDataError),
}

/// Parses the current ISO-8601 query format and Jellyfin's legacy compact UTC
/// timestamp used by older clients.
///
/// # Errors
///
/// Returns [`PlaystateError::InvalidDatePlayed`] for an invalid timestamp.
pub fn parse_date_played(value: &str) -> Result<DateTime<Utc>, PlaystateError> {
    if let Ok(date) = DateTime::parse_from_rfc3339(value.trim()) {
        return Ok(date.with_timezone(&Utc));
    }
    NaiveDateTime::parse_from_str(value.trim(), "%Y%m%d%H%M%S")
        .map(|date| date.and_utc())
        .map_err(|_| PlaystateError::InvalidDatePlayed)
}

/// Formats a UTC timestamp using Jellyfin's JSON date representation.
#[must_use]
pub fn format_date_played(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaystateUpdate {
    pub user_data: user_data::Model,
}

impl From<PlaystateUpdate> for UserItemDataDto {
    fn from(update: PlaystateUpdate) -> Self {
        user_data_to_dto(update.user_data)
    }
}

/// Coordinates authorization, item validation, and atomic playstate writes.
#[derive(Clone)]
pub struct PlaystateService {
    users: UserService,
    items: BaseItemRepository,
    user_data: UserDataRepository,
}

impl PlaystateService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            user_data: UserDataRepository::new(database),
        }
    }

    /// Marks an item played for the target user.
    ///
    /// # Errors
    ///
    /// Returns not-found, permission, or persistence errors after checking the
    /// target user and item in official controller order.
    pub async fn mark_played(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        date_played: Option<DateTime<Utc>>,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        self.validate_request(authenticated_user, target_user_id, item_id)
            .await?;
        let user_data = self
            .user_data
            .mark_played(item_id, target_user_id, &item_id.to_string(), date_played)
            .await?;
        Ok(PlaystateUpdate { user_data })
    }

    /// Marks an item played after the API layer has authorized the target user.
    ///
    /// This entry point exists for API-key authentication, which has
    /// administrator-equivalent access but no user model of its own.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence errors after checking the target user
    /// and item in official controller order.
    pub async fn mark_played_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        date_played: Option<DateTime<Utc>>,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        self.validate_authorized_request(target_user_id, item_id)
            .await?;
        let user_data = self
            .user_data
            .mark_played(item_id, target_user_id, &item_id.to_string(), date_played)
            .await?;
        Ok(PlaystateUpdate { user_data })
    }

    /// Marks an item unplayed for the target user.
    ///
    /// # Errors
    ///
    /// Returns not-found, permission, or persistence errors after checking the
    /// target user and item in official controller order.
    pub async fn mark_unplayed(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        self.validate_request(authenticated_user, target_user_id, item_id)
            .await?;
        let user_data = self
            .user_data
            .mark_unplayed(item_id, target_user_id, &item_id.to_string())
            .await?;
        Ok(PlaystateUpdate { user_data })
    }

    /// Marks an item unplayed after the API layer has authorized the target user.
    ///
    /// This entry point exists for API-key authentication, which has
    /// administrator-equivalent access but no user model of its own.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence errors after checking the target user
    /// and item in official controller order.
    pub async fn mark_unplayed_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        self.validate_authorized_request(target_user_id, item_id)
            .await?;
        let user_data = self
            .user_data
            .mark_unplayed(item_id, target_user_id, &item_id.to_string())
            .await?;
        Ok(PlaystateUpdate { user_data })
    }

    async fn validate_request(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<(), PlaystateError> {
        self.validate_target_user(target_user_id).await?;
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(PlaystateError::Forbidden);
        }
        self.validate_item(item_id).await
    }

    async fn validate_authorized_request(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<(), PlaystateError> {
        self.validate_target_user(target_user_id).await?;
        self.validate_item(item_id).await
    }

    async fn validate_target_user(&self, target_user_id: Uuid) -> Result<(), PlaystateError> {
        match self.users.get(target_user_id).await {
            Ok(_) => Ok(()),
            Err(UserError::NotFound) => Err(PlaystateError::UserNotFound),
            Err(error) => Err(error.into()),
        }
    }

    async fn validate_item(&self, item_id: Uuid) -> Result<(), PlaystateError> {
        self.items
            .get(item_id)
            .await?
            .ok_or(PlaystateError::ItemNotFound)?;
        Ok(())
    }
}
