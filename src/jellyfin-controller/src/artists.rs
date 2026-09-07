use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueCounts, ItemValueError, ItemValueInfo,
    ItemValueQuery, ItemValueRepository,
    entities::{base_item, item_value, user},
};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

use crate::{ItemByNameError, ItemByNameService, UserError, UserService};

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
    pub counts: ItemValueCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistPage {
    pub artists: Vec<Artist>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistDetail {
    pub item: base_item::Model,
    pub item_count: u64,
    pub counts: ItemValueCounts,
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
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

#[derive(Clone)]
pub struct ArtistService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
    item_by_name: ItemByNameService,
}

impl ArtistService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            item_values: ItemValueRepository::new(std::sync::Arc::clone(&database)),
            item_by_name: ItemByNameService::new(database),
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

    /// Resolves a Jellyfin music artist by display name.
    ///
    /// A persisted exact raw-name `MusicArtist` is preferred; otherwise the
    /// official deterministic accessed-by-name item is created and persisted.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
        query: ItemValueQuery,
    ) -> Result<ArtistDetail, ArtistError> {
        Self::authorize_target_user(authenticated_user, target_user_id)?;
        let requested_name = name.trim();
        if requested_name.is_empty() {
            return Err(ArtistError::NotFound);
        }
        let item = self
            .item_by_name
            .resolve_music_artist(requested_name)
            .await?;
        let (item_count, counts) = match item.name.as_deref() {
            Some(name) => self.item_values.music_artist_counts(name, &query).await?,
            None => (0, ItemValueCounts::default()),
        };
        Ok(ArtistDetail {
            item,
            item_count,
            counts,
        })
    }

    /// Resolves the persisted `MusicArtist` item that owns image metadata.
    ///
    /// This intentionally differs from [`Self::get`], whose public DTO ID may
    /// come from `item_values` and therefore is not a `base_items` foreign key.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, ArtistError> {
        Ok(self.items.get_by_type_and_name("MusicArtist", name).await?)
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
        self.list_authorized(kind, query).await
    }

    /// Lists artists after the caller has authorized and applied a target-user policy.
    ///
    /// # Errors
    ///
    /// Returns reconciliation, validation, or persistence errors.
    pub async fn list_authorized(
        &self,
        kind: ArtistValueKind,
        query: ItemValueQuery,
    ) -> Result<ArtistPage, ArtistError> {
        let mut query = self.scope_parent(query).await?;
        query.by_name_item_type = Some("MusicArtist".to_owned());
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

    fn authorize_target_user(
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), ArtistError> {
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
            counts: value.counts,
        }
    }
}
