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

const MUSIC_ITEM_TYPES: [&str; 4] = ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
    pub kind: GenreKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenreKind {
    Genre,
    MusicGenre,
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
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Clone)]
pub struct GenreService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
}

impl GenreService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
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
            kind: GenreKind::Genre,
        })
    }

    /// Resolves the persisted `Genre` item that owns image metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, GenreError> {
        Ok(self.items.get_by_type_and_name("Genre", name).await?)
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
        let (mut query, kind) = self.scope_parent(query).await?;
        query.by_name_item_type = Some(
            match kind {
                GenreKind::Genre => "Genre",
                GenreKind::MusicGenre => "MusicGenre",
            }
            .to_owned(),
        );
        let query = match kind {
            GenreKind::Genre => generic_genre_query(query),
            GenreKind::MusicGenre => music_genre_query(query),
        };
        let page = self
            .item_values
            .query_values(item_value::ItemValueType::Genre, &query)
            .await?;
        Ok(GenrePage {
            genres: page
                .values
                .into_iter()
                .map(|value| Genre::from_value(value, kind))
                .collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn scope_parent(
        &self,
        mut query: ItemValueQuery,
    ) -> Result<(ItemValueQuery, GenreKind), GenreError> {
        let Some(parent_id) = query.parent_id else {
            return Ok((query, GenreKind::Genre));
        };
        let parent = self
            .items
            .get(parent_id)
            .await?
            .ok_or(GenreError::NotFound)?;
        let kind = if is_music_collection_folder(&parent) {
            GenreKind::MusicGenre
        } else {
            GenreKind::Genre
        };
        if parent.is_folder {
            query.recursive = true;
        } else {
            query.parent_id = None;
            query.recursive = false;
            query.ids = vec![parent_id];
        }
        Ok((query, kind))
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
        Self::from_value(value, GenreKind::Genre)
    }
}

impl Genre {
    fn from_value(value: ItemValueInfo, kind: GenreKind) -> Self {
        Self {
            id: value.id,
            name: value.value,
            item_count: value.item_count,
            kind,
        }
    }
}

fn generic_genre_query(mut query: ItemValueQuery) -> ItemValueQuery {
    query
        .exclude_item_types
        .extend(MUSIC_ITEM_TYPES.iter().map(ToString::to_string));
    query
}

fn music_genre_query(mut query: ItemValueQuery) -> ItemValueQuery {
    if query.include_item_types.is_empty() {
        query.include_item_types = MUSIC_ITEM_TYPES.iter().map(ToString::to_string).collect();
    } else {
        query
            .include_item_types
            .retain(|item_type| is_music_item_type(item_type));
        if query.include_item_types.is_empty() {
            query
                .include_item_types
                .push("__jellyfin_no_music_item_type__".to_owned());
        }
    }
    query
}

fn is_music_collection_folder(item: &base_item::Model) -> bool {
    item.is_folder
        && item
            .data
            .as_ref()
            .and_then(|data| {
                collection_type(
                    data,
                    &["CollectionType", "collectionType", "collection_type"],
                )
            })
            .is_some_and(|collection_type| {
                collection_type.eq_ignore_ascii_case("music")
                    || collection_type.eq_ignore_ascii_case("musicvideos")
            })
}

fn collection_type<'a>(data: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    let object = data.as_object()?;
    keys.iter().find_map(|key| object.get(*key)?.as_str())
}

fn is_music_item_type(candidate: &str) -> bool {
    MUSIC_ITEM_TYPES.iter().any(|item_type| {
        candidate.eq_ignore_ascii_case(item_type) || candidate.ends_with(&format!(".{item_type}"))
    })
}

fn virtual_genre(name: &str) -> Genre {
    Genre {
        id: jellyfin_genre_id(name),
        name: name.to_owned(),
        item_count: 0,
        kind: GenreKind::Genre,
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
