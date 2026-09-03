use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, ProductionYearOrder,
    entities::{base_item, user},
};
use md5::{Digest, Md5};
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

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
}

#[derive(Clone)]
pub struct YearService {
    users: UserService,
    items: BaseItemRepository,
}

impl YearService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(database),
        }
    }

    /// Resolves a Jellyfin year item for a positive production year.
    ///
    /// Persisted `Year` items win. If none exists but `PostgreSQL` finds at
    /// least one item tagged with the requested production year, the service
    /// returns the virtual item-by-name shape used by Jellyfin.
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
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        if year <= 0 {
            return Err(YearError::NotFound);
        }
        if let Some(item) = self.items.year_item(year).await? {
            return Ok(YearItem::Persisted(item));
        }
        if !self.items.has_production_year(year).await? {
            return Err(YearError::NotFound);
        }
        let name = year.to_string();
        Ok(YearItem::Virtual(Year {
            id: jellyfin_year_id(&name),
            name,
        }))
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
        let page = self.items.production_years(&query, order).await?;
        Ok(YearPage {
            years: page
                .years
                .into_iter()
                .map(|year| {
                    let name = year.to_string();
                    Year {
                        id: jellyfin_year_id(&name),
                        name,
                    }
                })
                .collect(),
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
}

fn jellyfin_year_id(name: &str) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(format!("Year-{name}").as_bytes());
    Uuid::from_bytes_le(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::jellyfin_year_id;

    #[test]
    fn virtual_year_ids_are_stable_jellyfin_style_md5_guids() {
        assert_eq!(
            jellyfin_year_id("2024").simple().to_string(),
            "e76befe12d5e74b5e1f9b9e239a6c8fa"
        );
    }
}
