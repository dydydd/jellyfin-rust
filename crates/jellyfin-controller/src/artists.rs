use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueError, ItemValueInfo, ItemValueQuery,
    ItemValueRepository,
    entities::{item_value, user},
};
use md5::{Digest, Md5};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistValueKind {
    Artist,
    AlbumArtist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistPage {
    pub artists: Vec<Artist>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Error)]
pub enum ArtistError {
    #[error("artist was not found")]
    NotFound,
    #[error("target user was not found")]
    UserNotFound,
    #[error("artist query is forbidden")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Clone)]
pub struct ArtistService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
}

impl ArtistService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            item_values: ItemValueRepository::new(database),
        }
    }

    /// Resolves a Jellyfin music artist by display name.
    ///
    /// Missing, non-empty names are returned as virtual item-by-name artists to
    /// match Jellyfin's current `ArtistsController.GetArtistByName` behavior.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<Artist, ArtistError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let requested_name = name.trim();
        if requested_name.is_empty() {
            return Err(ArtistError::NotFound);
        }
        for kind in [ArtistValueKind::Artist, ArtistValueKind::AlbumArtist] {
            let Some(value) = self.find_value(kind, requested_name).await? else {
                continue;
            };
            let item_count = self
                .item_values
                .items_for_value(kind.value_type(), &value.value)
                .await?
                .into_iter()
                .filter(|item| item.item_type != "PLACEHOLDER")
                .count();
            if item_count != 0 {
                return Ok(Artist {
                    id: value.item_value_id,
                    name: value.value,
                    item_count: u64::try_from(item_count).unwrap_or(u64::MAX),
                });
            }
        }
        Ok(virtual_artist(requested_name))
    }

    /// Lists Jellyfin artist or album-artist item-by-name values.
    ///
    /// # Errors
    ///
    /// Returns forbidden, user lookup, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        kind: ArtistValueKind,
        query: ItemValueQuery,
    ) -> Result<ArtistPage, ArtistError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let query = self.scope_parent(query).await?;
        let page = self
            .item_values
            .query_values(kind.value_type(), &query)
            .await?;
        Ok(ArtistPage {
            artists: page.values.into_iter().map(Artist::from).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn find_value(
        &self,
        kind: ArtistValueKind,
        name: &str,
    ) -> Result<Option<item_value::Model>, ArtistError> {
        match self.item_values.get_exact(kind.value_type(), name).await {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) | Err(ItemValueError::InvalidValue) => {}
            Err(error) => return Err(error.into()),
        }
        match self
            .item_values
            .get_normalized(kind.value_type(), name)
            .await
        {
            Ok(value) => Ok(value),
            Err(ItemValueError::InvalidValue) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn scope_parent(&self, mut query: ItemValueQuery) -> Result<ItemValueQuery, ArtistError> {
        let Some(parent_id) = query.parent_id else {
            return Ok(query);
        };
        let parent = self
            .items
            .get(parent_id)
            .await?
            .ok_or(ArtistError::NotFound)?;
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
    ) -> Result<(), ArtistError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(ArtistError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(ArtistError::Forbidden);
        }
        Ok(())
    }
}

impl ArtistValueKind {
    const fn value_type(self) -> item_value::ItemValueType {
        match self {
            Self::Artist => item_value::ItemValueType::Artist,
            Self::AlbumArtist => item_value::ItemValueType::AlbumArtist,
        }
    }
}

impl From<ItemValueInfo> for Artist {
    fn from(value: ItemValueInfo) -> Self {
        Self {
            id: value.id,
            name: value.value,
            item_count: value.item_count,
        }
    }
}

fn virtual_artist(name: &str) -> Artist {
    Artist {
        id: jellyfin_artist_id(name),
        name: name.to_owned(),
        item_count: 0,
    }
}

fn jellyfin_artist_id(name: &str) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(format!("Artist-{name}").as_bytes());
    Uuid::from_bytes_le(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::jellyfin_artist_id;

    #[test]
    fn virtual_artist_ids_are_stable_jellyfin_style_md5_guids() {
        assert_eq!(
            jellyfin_artist_id("ABBA").simple().to_string(),
            "cd831fa4290d5e825421e1f4e8da1fb6"
        );
    }
}
