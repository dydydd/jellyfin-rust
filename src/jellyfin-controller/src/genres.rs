use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueCounts, ItemValueError, ItemValueInfo,
    ItemValueQuery, ItemValueRepository,
    entities::{base_item, item_value, user},
};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

use crate::{ItemByNameError, ItemByNameKind, ItemByNameService, UserError, UserService};

const MUSIC_ITEM_TYPES: [&str; 4] = ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
    pub counts: ItemValueCounts,
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

#[derive(Debug, Clone, PartialEq)]
pub struct GenreDetail {
    pub item: base_item::Model,
    pub item_count: u64,
    pub counts: ItemValueCounts,
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
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

#[derive(Clone)]
pub struct GenreService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
    item_by_name: ItemByNameService,
}

impl GenreService {
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
            item_values: ItemValueRepository::new(std::sync::Arc::clone(&database)),
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

    /// Resolves a generic Jellyfin genre by display or slug name.
    ///
    /// Ordinary names create their deterministic persisted entity. Slug names
    /// only resolve existing persisted entities; a miss is returned to the API
    /// so it can emit Jellyfin's empty Genre DTO.
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
    ) -> Result<Option<GenreDetail>, GenreError> {
        self.authorize_target_user(authenticated_user, target_user_id)?;
        let Some(item) = self
            .item_by_name
            .resolve(ItemByNameKind::Genre, name)
            .await?
        else {
            return Ok(None);
        };
        let Some(name) = item.name.as_deref() else {
            return Ok(Some(GenreDetail {
                item,
                item_count: 0,
                counts: ItemValueCounts::default(),
            }));
        };
        let Some(value) = self
            .item_values
            .get_normalized(item_value::ItemValueType::Genre, name)
            .await?
        else {
            return Ok(Some(GenreDetail {
                item,
                item_count: 0,
                counts: ItemValueCounts::default(),
            }));
        };
        query.search_term = Some(value.value);
        let query = generic_genre_query(query);
        let page = self
            .item_values
            .query_values(item_value::ItemValueType::Genre, &query)
            .await?;
        let item_count = page
            .values
            .iter()
            .find(|candidate| candidate.id == value.item_value_id)
            .map_or(0, |candidate| candidate.item_count);
        let counts = page
            .values
            .into_iter()
            .find(|candidate| candidate.id == value.item_value_id)
            .map_or_else(ItemValueCounts::default, |candidate| candidate.counts);
        Ok(Some(GenreDetail {
            item,
            item_count,
            counts,
        }))
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
        self.list_authorized(query).await
    }

    /// Lists genres after the caller has authorized and applied a target-user policy.
    ///
    /// # Errors
    ///
    /// Returns reconciliation, validation, or persistence errors.
    pub async fn list_authorized(&self, query: ItemValueQuery) -> Result<GenrePage, GenreError> {
        self.item_by_name.reconcile_once().await?;
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
            .query_persisted_item_by_name_values(
                item_value::ItemValueType::Genre,
                match kind {
                    GenreKind::Genre => "Genre",
                    GenreKind::MusicGenre => "MusicGenre",
                },
                &query,
            )
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

    fn authorize_target_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), GenreError> {
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
            counts: value.counts,
            kind,
        }
    }
}

fn generic_genre_query(mut query: ItemValueQuery) -> ItemValueQuery {
    query
        .discovery_exclude_item_types
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
