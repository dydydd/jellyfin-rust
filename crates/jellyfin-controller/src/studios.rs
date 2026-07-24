use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueError, ItemValueInfo, ItemValueQuery,
    ItemValueRepository,
    entities::{base_item, item_value, user},
};
use md5::{Digest, Md5};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Studio {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudioPage {
    pub studios: Vec<Studio>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Error)]
pub enum StudioError {
    #[error("studio was not found")]
    NotFound,
    #[error("target user was not found")]
    UserNotFound,
    #[error("studio query is forbidden")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Clone)]
pub struct StudioService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
}

impl StudioService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            item_values: ItemValueRepository::new(database),
        }
    }

    /// Resolves a Jellyfin studio by display name.
    ///
    /// Missing, non-empty names are returned as virtual item-by-name studios to
    /// match Jellyfin's current `StudiosController.GetStudio` behavior.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<Studio, StudioError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let requested_name = name.trim();
        if requested_name.is_empty() {
            return Err(StudioError::NotFound);
        }
        let Some(value) = self.find_value(requested_name).await? else {
            return Ok(virtual_studio(requested_name));
        };
        let item_count = self
            .item_values
            .items_for_value(item_value::ItemValueType::Studios, &value.value)
            .await?
            .into_iter()
            .filter(|item| item.item_type != "PLACEHOLDER")
            .count();
        if item_count == 0 {
            return Ok(virtual_studio(&value.value));
        }
        Ok(Studio {
            id: value.item_value_id,
            name: value.value,
            item_count: u64::try_from(item_count).unwrap_or(u64::MAX),
        })
    }

    /// Resolves the persisted `Studio` item that owns image metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, StudioError> {
        Ok(self.items.get_by_type_and_name("Studio", name).await?)
    }

    /// Lists Jellyfin studios attached to filtered items.
    ///
    /// # Errors
    ///
    /// Returns forbidden, user lookup, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: ItemValueQuery,
    ) -> Result<StudioPage, StudioError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let mut query = self.scope_parent(query).await?;
        query.by_name_item_type = Some("Studio".to_owned());
        let page = self
            .item_values
            .query_values(item_value::ItemValueType::Studios, &query)
            .await?;
        Ok(StudioPage {
            studios: page.values.into_iter().map(Studio::from).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn find_value(&self, name: &str) -> Result<Option<item_value::Model>, StudioError> {
        match self
            .item_values
            .get_exact(item_value::ItemValueType::Studios, name)
            .await
        {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) | Err(ItemValueError::InvalidValue) => {}
            Err(error) => return Err(error.into()),
        }
        match self
            .item_values
            .get_normalized(item_value::ItemValueType::Studios, name)
            .await
        {
            Ok(value) => Ok(value),
            Err(ItemValueError::InvalidValue) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn scope_parent(&self, mut query: ItemValueQuery) -> Result<ItemValueQuery, StudioError> {
        let Some(parent_id) = query.parent_id else {
            return Ok(query);
        };
        let parent = self
            .items
            .get(parent_id)
            .await?
            .ok_or(StudioError::NotFound)?;
        if parent.is_folder {
            query.recursive = true;
        } else {
            query.parent_id = None;
            query.recursive = false;
            query.ids = vec![parent_id];
        }
        Ok(query)
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), StudioError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(StudioError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(StudioError::Forbidden);
        }
        Ok(())
    }
}

impl From<ItemValueInfo> for Studio {
    fn from(value: ItemValueInfo) -> Self {
        Self {
            id: value.id,
            name: value.value,
            item_count: value.item_count,
        }
    }
}

fn virtual_studio(name: &str) -> Studio {
    Studio {
        id: jellyfin_studio_id(name),
        name: name.to_owned(),
        item_count: 0,
    }
}

fn jellyfin_studio_id(name: &str) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(format!("Studio-{name}").as_bytes());
    Uuid::from_bytes_le(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::jellyfin_studio_id;

    #[test]
    fn virtual_studio_ids_are_stable_jellyfin_style_md5_guids() {
        assert_eq!(
            jellyfin_studio_id("Pixar").simple().to_string(),
            "b5297c03aa4144a5e71131cdd4d79122"
        );
    }
}
