use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueCounts, ItemValueError, ItemValueInfo,
    ItemValueQuery, ItemValueRepository,
    entities::{base_item, item_value, user},
};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

use crate::{ItemByNameError, ItemByNameKind, ItemByNameService, UserError, UserService};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Studio {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
    pub counts: ItemValueCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudioPage {
    pub studios: Vec<Studio>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudioDetail {
    pub item: base_item::Model,
    pub item_count: u64,
    pub counts: ItemValueCounts,
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
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

#[derive(Clone)]
pub struct StudioService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
    item_by_name: ItemByNameService,
}

impl StudioService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        let item_by_name = ItemByNameService::new(std::sync::Arc::clone(&database));
        Self::with_item_by_name_service(database, item_by_name)
    }

    #[must_use]
    pub fn with_item_by_name_service(
        database: impl Into<jellyfin_data::SharedDatabase>,
        item_by_name: ItemByNameService,
    ) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            item_values: ItemValueRepository::new(database),
            item_by_name,
        }
    }

    pub fn set_item_by_name_directories(
        &self,
        program_data_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) {
        self.item_by_name
            .set_directories(program_data_directory, internal_metadata_directory);
    }

    /// Resolves a Jellyfin studio by display name.
    ///
    /// Every name, including one containing a hyphen, is resolved through the
    /// official deterministic persisted Studio path.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
        mut query: ItemValueQuery,
    ) -> Result<StudioDetail, StudioError> {
        self.authorize_target_user(authenticated_user, target_user_id)?;
        let item = self
            .item_by_name
            .resolve_direct(ItemByNameKind::Studio, name)
            .await?;
        let Some(item_name) = item.name.as_deref() else {
            return Ok(StudioDetail {
                item,
                item_count: 0,
                counts: ItemValueCounts::default(),
            });
        };
        let Some(value) = self.find_value(item_name).await? else {
            return Ok(StudioDetail {
                item,
                item_count: 0,
                counts: ItemValueCounts::default(),
            });
        };
        query.search_term = Some(value.value.clone());
        let candidate = self
            .item_values
            .query_values(item_value::ItemValueType::Studios, &query)
            .await?
            .values
            .into_iter()
            .find(|candidate| candidate.id == value.item_value_id);
        Ok(StudioDetail {
            item,
            item_count: candidate.as_ref().map_or(0, |value| value.item_count),
            counts: candidate.map_or_else(ItemValueCounts::default, |value| value.counts),
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
        self.list_authorized(query).await
    }

    /// Lists studios after the caller has authorized and applied a target-user policy.
    ///
    /// # Errors
    ///
    /// Returns reconciliation, validation, or persistence errors.
    pub async fn list_authorized(&self, query: ItemValueQuery) -> Result<StudioPage, StudioError> {
        self.item_by_name.reconcile_studios_and_years_once().await?;
        let mut query = self.scope_parent(query).await?;
        query.by_name_item_type = Some("Studio".to_owned());
        let page = self
            .item_values
            .query_persisted_item_by_name_values(
                item_value::ItemValueType::Studios,
                "Studio",
                &query,
            )
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

    fn authorize_target_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), StudioError> {
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
            counts: value.counts,
        }
    }
}
