use jellyfin_data::{
    ItemValueError, ItemValueInfo, ItemValueQuery, ItemValueRepository,
    entities::{item_value, user},
};
use md5::{Digest, Md5};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

const MUSIC_ITEM_TYPES: [&str; 4] = ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenrePage {
    pub genres: Vec<Genre>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Error)]
pub enum GenreError {
    #[error("genre was not found")]
    NotFound,
    #[error("target user was not found")]
    UserNotFound,
    #[error("genre query is forbidden")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Clone)]
pub struct GenreService {
    users: UserService,
    item_values: ItemValueRepository,
}

impl GenreService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            item_values: ItemValueRepository::new(database),
        }
    }

    /// Resolves a generic Jellyfin genre by display or slug name.
    ///
    /// Missing, non-empty names are returned as virtual item-by-name genres to
    /// match Jellyfin's current `GenresController.GetGenre` behavior.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<Genre, GenreError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let requested_name = name.trim();
        if requested_name.is_empty() {
            return Err(GenreError::NotFound);
        }
        let value = self.find_value(requested_name).await?;
        let Some(value) = value else {
            return Ok(virtual_genre(requested_name));
        };
        let page = self
            .item_values
            .query_values(
                item_value::ItemValueType::Genre,
                &generic_genre_query(ItemValueQuery {
                    search_term: Some(value.value.clone()),
                    ..ItemValueQuery::default()
                }),
            )
            .await?;
        let item_count = page
            .values
            .into_iter()
            .find(|candidate| candidate.id == value.item_value_id)
            .map_or(0, |candidate| candidate.item_count);
        if item_count == 0 {
            return Ok(virtual_genre(&value.value));
        }
        Ok(Genre {
            id: value.item_value_id,
            name: value.value,
            item_count,
        })
    }

    /// Lists generic Jellyfin genres attached to filtered non-music items.
    ///
    /// # Errors
    ///
    /// Returns forbidden, user lookup, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: ItemValueQuery,
    ) -> Result<GenrePage, GenreError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let page = self
            .item_values
            .query_values(
                item_value::ItemValueType::Genre,
                &generic_genre_query(query),
            )
            .await?;
        Ok(GenrePage {
            genres: page.values.into_iter().map(Genre::from).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn find_value(&self, name: &str) -> Result<Option<item_value::Model>, GenreError> {
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
    ) -> Result<(), GenreError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(GenreError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(GenreError::Forbidden);
        }
        Ok(())
    }
}

impl From<ItemValueInfo> for Genre {
    fn from(value: ItemValueInfo) -> Self {
        Self {
            id: value.id,
            name: value.value,
            item_count: value.item_count,
        }
    }
}

fn generic_genre_query(mut query: ItemValueQuery) -> ItemValueQuery {
    query
        .exclude_item_types
        .extend(MUSIC_ITEM_TYPES.iter().map(ToString::to_string));
    query
}

fn virtual_genre(name: &str) -> Genre {
    Genre {
        id: jellyfin_genre_id(name),
        name: name.to_owned(),
        item_count: 0,
    }
}

fn jellyfin_genre_id(name: &str) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(format!("Genre-{name}").as_bytes());
    Uuid::from_bytes_le(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::jellyfin_genre_id;

    #[test]
    fn virtual_genre_ids_are_stable_jellyfin_style_md5_guids() {
        assert_eq!(
            jellyfin_genre_id("Drama").simple().to_string(),
            "7ddf95d8ffa3c974f9c81b4c7d6c4f54"
        );
    }
}
