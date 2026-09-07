use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, ProductionYearOrder,
    entities::{base_item, user},
};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

use crate::{ItemByNameError, ItemByNameKind, ItemByNameService, UserError, UserService};

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum YearItem {
    Persisted(base_item::Model),
    Virtual(Year),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Year {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YearPage {
    pub years: Vec<Year>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Error)]
pub enum YearError {
    #[error("year must be greater than zero")]
    InvalidYear,
    #[error("year was not found")]
    NotFound,
    #[error("target user was not found")]
    UserNotFound,
    #[error("year query is forbidden")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

#[derive(Clone)]
pub struct YearService {
    users: UserService,
    items: BaseItemRepository,
    item_by_name: ItemByNameService,
}

impl YearService {
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
            items: BaseItemRepository::new(database),
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

    /// Resolves a Jellyfin year item for a positive year.
    ///
    /// Every positive value resolves through Jellyfin's deterministic
    /// persisted Year path, even when no library item currently uses it.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        year: i32,
    ) -> Result<YearItem, YearError> {
        self.authorize_target_user(authenticated_user, target_user_id)?;
        if year <= 0 {
            return Err(YearError::InvalidYear);
        }
        Ok(YearItem::Persisted(
            self.item_by_name
                .resolve_direct(ItemByNameKind::Year, &year.to_string())
                .await?,
        ))
    }

    /// Lists distinct production years visible through the requested query.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: BaseItemQuery,
        order: ProductionYearOrder,
    ) -> Result<YearPage, YearError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.list_authorized(query, order).await
    }

    /// Lists production years after the caller has already loaded and
    /// authorized the target user while applying its library policy.
    ///
    /// This keeps the public [`Self::list`] authorization boundary for normal
    /// callers, while avoiding a second `users` read in endpoints that need
    /// the policy-filtered query before listing years.
    ///
    /// # Errors
    ///
    /// Returns reconciliation or persistence errors.
    pub async fn list_authorized(
        &self,
        query: BaseItemQuery,
        order: ProductionYearOrder,
    ) -> Result<YearPage, YearError> {
        self.item_by_name.reconcile_studios_and_years_once().await?;
        let page = self.items.production_years(&query, order).await?;
        let names = page
            .years
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let persisted = self
            .item_by_name
            .ensure_many_direct(ItemByNameKind::Year, &names)
            .await?;
        Ok(YearPage {
            years: page
                .years
                .into_iter()
                .map(|year| -> Result<Year, YearError> {
                    let name = year.to_string();
                    Ok(Year {
                        id: persisted.get(&name).ok_or(YearError::NotFound)?.id,
                        name,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), YearError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(YearError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(YearError::Forbidden);
        }
        Ok(())
    }

    fn authorize_target_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), YearError> {
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(YearError::Forbidden);
        }
        Ok(())
    }
}
