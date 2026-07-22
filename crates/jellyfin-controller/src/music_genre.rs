use jellyfin_data::{
    ItemValueError, ItemValueRepository,
    entities::{base_item, item_value, user},
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

#[derive(Debug, Error)]
pub enum MusicGenreError {
    #[error("target user not found")]
    UserNotFound,
    #[error("music genre not found")]
    NotFound,
    #[error("the authenticated user cannot access this user's music genres")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicGenre {
    pub id: Uuid,
    pub name: String,
    pub item_count: usize,
}

/// Resolves persisted music genres and coordinates optional target-user
/// authorization for the API.
#[derive(Clone)]
pub struct MusicGenreService {
    users: UserService,
    item_values: ItemValueRepository,
}

impl MusicGenreService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            item_values: ItemValueRepository::new(database),
        }
    }

    /// Resolves a music genre by exact, legacy slug, or normalized name.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<MusicGenre, MusicGenreError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let value = self
            .find_value(name)
            .await?
            .ok_or(MusicGenreError::NotFound)?;
        let linked_items = self
            .item_values
            .items_for_value(item_value::ItemValueType::Genre, &value.value)
            .await?;
        let item_count = linked_items
            .iter()
            .filter(|item| is_music_item(item))
            .count();
        if item_count == 0 {
            return Err(MusicGenreError::NotFound);
        }
        Ok(MusicGenre {
            id: value.item_value_id,
            name: value.value,
            item_count,
        })
    }

    async fn find_value(&self, name: &str) -> Result<Option<item_value::Model>, MusicGenreError> {
        let mut candidates = vec![name.to_owned()];
        if name.contains('-') {
            candidates
                .extend(['&', '/', '?'].map(|separator| name.replace('-', &separator.to_string())));
        }
        for candidate in &candidates {
            match self
                .item_values
                .get_exact(item_value::ItemValueType::Genre, candidate)
                .await
            {
                Ok(Some(value)) => return Ok(Some(value)),
                Ok(None) | Err(ItemValueError::InvalidValue) => {}
                Err(error) => return Err(error.into()),
            }
        }
        for candidate in candidates {
            match self
                .item_values
                .get_normalized(item_value::ItemValueType::Genre, &candidate)
                .await
            {
                Ok(Some(value)) => return Ok(Some(value)),
                Ok(None) | Err(ItemValueError::InvalidValue) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), MusicGenreError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(MusicGenreError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(MusicGenreError::Forbidden);
        }
        Ok(())
    }
}

fn is_music_item(item: &base_item::Model) -> bool {
    ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"]
        .iter()
        .any(|item_type| {
            item.item_type.eq_ignore_ascii_case(item_type)
                || item.item_type.ends_with(&format!(".{item_type}"))
        })
}
