use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueCounts, ItemValueError, ItemValueInfo,
    ItemValueQuery, ItemValueRepository,
    entities::{base_item, item_value, user},
};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

use crate::{ItemByNameError, ItemByNameKind, ItemByNameService, UserError, UserService};

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
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicGenre {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
    pub counts: ItemValueCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicGenrePage {
    pub genres: Vec<MusicGenre>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MusicGenreDetail {
    pub item: base_item::Model,
    pub item_count: u64,
    pub counts: ItemValueCounts,
}

/// Resolves persisted music genres and coordinates optional target-user
/// authorization for the API.
#[derive(Clone)]
pub struct MusicGenreService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
    item_by_name: ItemByNameService,
}

impl MusicGenreService {
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

    /// Resolves a persisted music genre by official direct-name or slug rules.
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
    ) -> Result<MusicGenreDetail, MusicGenreError> {
        self.authorize_target_user(authenticated_user, target_user_id)?;
        let item = self
            .item_by_name
            .resolve(ItemByNameKind::MusicGenre, name)
            .await?
            .ok_or(MusicGenreError::NotFound)?;
        let Some(name) = item.name.as_deref() else {
            return Ok(MusicGenreDetail {
                item,
                item_count: 0,
                counts: ItemValueCounts::default(),
            });
        };
        let Some(value) = self
            .item_values
            .get_normalized(item_value::ItemValueType::Genre, name)
            .await?
        else {
            return Ok(MusicGenreDetail {
                item,
                item_count: 0,
                counts: ItemValueCounts::default(),
            });
        };
        query.search_term = Some(value.value);
        query.include_item_types = MUSIC_ITEM_TYPES.iter().map(ToString::to_string).collect();
        let candidate = self
            .item_values
            .query_values(item_value::ItemValueType::Genre, &query)
            .await?
            .values
            .into_iter()
            .find(|candidate| candidate.id == value.item_value_id);
        Ok(MusicGenreDetail {
            item,
            item_count: candidate.as_ref().map_or(0, |value| value.item_count),
            counts: candidate.map_or_else(ItemValueCounts::default, |value| value.counts),
        })
    }

    /// Resolves the persisted `MusicGenre` item that owns image metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(
        &self,
        name: &str,
    ) -> Result<Option<base_item::Model>, MusicGenreError> {
        Ok(self.items.get_by_type_and_name("MusicGenre", name).await?)
    }

    /// Lists music genres attached to filtered music items.
    ///
    /// # Errors
    ///
    /// Returns forbidden, user lookup, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: ItemValueQuery,
    ) -> Result<MusicGenrePage, MusicGenreError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.list_authorized(query).await
    }

    /// Lists music genres after the caller has authorized and applied a target-user policy.
    ///
    /// # Errors
    ///
    /// Returns reconciliation, validation, or persistence errors.
    pub async fn list_authorized(
        &self,
        query: ItemValueQuery,
    ) -> Result<MusicGenrePage, MusicGenreError> {
        self.item_by_name.reconcile_once().await?;
        let mut query = self.scope_music_query(query).await?;
        query.by_name_item_type = Some("MusicGenre".to_owned());
        let page = self
            .item_values
            .query_persisted_item_by_name_values(
                item_value::ItemValueType::Genre,
                "MusicGenre",
                &query,
            )
            .await?;
        Ok(MusicGenrePage {
            genres: page.values.into_iter().map(MusicGenre::from).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
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

    fn authorize_target_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), MusicGenreError> {
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(MusicGenreError::Forbidden);
        }
        Ok(())
    }

    async fn scope_music_query(
        &self,
        mut query: ItemValueQuery,
    ) -> Result<ItemValueQuery, MusicGenreError> {
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
        let Some(parent_id) = query.parent_id else {
            return Ok(query);
        };
        let parent = self
            .items
            .get(parent_id)
            .await?
            .ok_or(MusicGenreError::NotFound)?;
        if parent.is_folder {
            query.recursive = true;
        } else {
            query.parent_id = None;
            query.recursive = false;
            query.ids = vec![parent_id];
        }
        Ok(query)
    }
}

impl From<ItemValueInfo> for MusicGenre {
    fn from(value: ItemValueInfo) -> Self {
        Self {
            id: value.id,
            name: value.value,
            item_count: value.item_count,
            counts: value.counts,
        }
    }
}

const MUSIC_ITEM_TYPES: [&str; 4] = ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"];

fn is_music_item_type(candidate: &str) -> bool {
    MUSIC_ITEM_TYPES.iter().any(|item_type| {
        candidate.eq_ignore_ascii_case(item_type) || candidate.ends_with(&format!(".{item_type}"))
    })
}
