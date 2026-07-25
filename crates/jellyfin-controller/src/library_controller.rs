use jellyfin_data::{
    BaseItemCounts, BaseItemError, BaseItemPage, BaseItemQuery, BaseItemRepository, ItemValueError,
    ItemValueRepository, PlaylistRepository, PlaylistStoreError,
    entities::{base_item, item_value, user},
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{HydratedBaseItem, ItemTypeRegistry, UserError, UserService};

#[derive(Debug, Error)]
pub enum LibraryControllerError {
    #[error("target user was not found")]
    UserNotFound,
    #[error("library item was not found")]
    ItemNotFound,
    #[error("administrator access is required")]
    Forbidden,
    #[error("library item has no downloadable file")]
    FileNotFound,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error(transparent)]
    Playlist(#[from] PlaylistStoreError),
}

/// Coordinates `LibraryController` authorization with persisted item queries.
#[derive(Clone)]
pub struct LibraryControllerService {
    users: UserService,
    items: BaseItemRepository,
    item_types: ItemTypeRegistry,
    item_values: ItemValueRepository,
    playlists: PlaylistRepository,
}

impl LibraryControllerService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self::with_item_type_registry(database, ItemTypeRegistry::default())
    }

    #[must_use]
    pub fn with_item_type_registry(
        database: DatabaseConnection,
        item_types: ItemTypeRegistry,
    ) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            item_values: ItemValueRepository::new(database.clone()),
            playlists: PlaylistRepository::new(database),
            item_types,
        }
    }

    /// Loads an item after validating access to the requested user context.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn item(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, LibraryControllerError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.items
            .get(item_id)
            .await?
            .and_then(|item| self.item_types.hydrate(item))
            .map(HydratedBaseItem::into_model)
            .ok_or(LibraryControllerError::ItemNotFound)
    }

    /// Returns persisted ancestors nearest-first.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn ancestors(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<Vec<base_item::Model>, LibraryControllerError> {
        self.item(authenticated_user, target_user_id, item_id)
            .await?;
        Ok(self
            .items
            .ancestors(item_id)
            .await?
            .into_iter()
            .filter_map(|entry| self.item_types.hydrate(entry.item))
            .map(HydratedBaseItem::into_model)
            .collect())
    }

    /// Returns visible collections containing one item, ordered like Jellyfin.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn collections_containing_item(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        start_index: u64,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.item(authenticated_user, target_user_id, item_id)
            .await?;
        let page = self
            .items
            .collections_containing_item(item_id, start_index, limit)
            .await?;
        Ok(self.hydrate_page(page))
    }

    /// Resolves the persisted file path for a downloadable item.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, missing-file, or persistence errors.
    pub async fn download_path(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<String, LibraryControllerError> {
        self.item(authenticated_user, target_user_id, item_id)
            .await?
            .path
            .filter(|path| !path.is_empty())
            .ok_or(LibraryControllerError::FileNotFound)
    }

    /// Finds persisted non-virtual items with the same item and media types.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn similar_items(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        let media_types = item.media_type.into_iter().collect();
        let page = self
            .items
            .query(&BaseItemQuery {
                exclude_ids: vec![item.id],
                include_item_types: vec![item.item_type],
                media_types,
                is_virtual_item: Some(false),
                limit,
                ..Default::default()
            })
            .await?;
        Ok(self.hydrate_page(page))
    }

    /// Creates a random audio mix from the seed item's normalized genres.
    ///
    /// Audio seeds remain first, while all other candidates are selected by
    /// PostgreSQL from shared genre mappings up to Jellyfin's 200-item cap.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, invalid-playlist, or persistence errors.
    pub async fn instant_mix(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        if item.item_type == "Playlist" {
            let playlist = self
                .playlists
                .get(item_id)
                .await?
                .ok_or(LibraryControllerError::ItemNotFound)?;
            if !playlist.open_access
                && playlist.owner_user_id != Some(target_user_id)
                && !playlist
                    .shares
                    .iter()
                    .any(|share| share.user_id == target_user_id)
            {
                return Err(LibraryControllerError::ItemNotFound);
            }
        }
        let genres = self
            .item_values
            .values_for_item(item_id, item_value::ItemValueType::Genre)
            .await?;
        let genre_ids = genres
            .into_iter()
            .map(|genre| genre.item_value_id)
            .collect::<Vec<_>>();
        let seed_is_audio = item.item_type == "Audio";
        let mut items = self
            .item_values
            .random_audio_for_genres(&genre_ids, 201)
            .await?;
        items.retain(|candidate| candidate.id != item.id);
        items.truncate(200_usize.saturating_sub(usize::from(seed_is_audio)));
        if seed_is_audio {
            items.insert(0, item);
        }
        let total_record_count = u64::try_from(items.len()).unwrap_or(u64::MAX);
        items.truncate(
            usize::try_from(limit.unwrap_or(total_record_count))
                .unwrap_or(usize::MAX)
                .min(items.len()),
        );
        Ok(BaseItemPage {
            items: items
                .into_iter()
                .filter_map(|item| self.item_types.hydrate(item))
                .map(HydratedBaseItem::into_model)
                .collect(),
            total_record_count,
            start_index: 0,
        })
    }

    /// Creates a random audio mix from one normalized genre identifier.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn instant_mix_for_genre(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        genre_id: Uuid,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.item_values
            .get_by_id(genre_id, item_value::ItemValueType::Genre)
            .await?
            .ok_or(LibraryControllerError::ItemNotFound)?;
        let mut items = self
            .item_values
            .random_audio_for_genres(&[genre_id], 200)
            .await?;
        let total_record_count = u64::try_from(items.len()).unwrap_or(u64::MAX);
        items.truncate(
            usize::try_from(limit.unwrap_or(total_record_count))
                .unwrap_or(usize::MAX)
                .min(items.len()),
        );
        Ok(BaseItemPage {
            items: items
                .into_iter()
                .filter_map(|item| self.item_types.hydrate(item))
                .map(HydratedBaseItem::into_model)
                .collect(),
            total_record_count,
            start_index: 0,
        })
    }

    /// Creates a random audio mix from a normalized genre name.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn instant_mix_for_genre_name(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        genre_name: &str,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let genre = self
            .item_values
            .get_normalized(item_value::ItemValueType::Genre, genre_name)
            .await?
            .ok_or(LibraryControllerError::ItemNotFound)?;
        self.instant_mix_for_genre(
            authenticated_user,
            target_user_id,
            genre.item_value_id,
            limit,
        )
        .await
    }

    /// Counts non-virtual library items, optionally scoped to a user's favorite state.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when the aggregate query fails.
    pub async fn item_counts(
        &self,
        user_id: Option<Uuid>,
        is_favorite: Option<bool>,
    ) -> Result<BaseItemCounts, LibraryControllerError> {
        Ok(self.items.item_counts(user_id, is_favorite).await?)
    }

    /// Atomically deletes complete item subtrees for an administrator.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-found, protected-item, or persistence errors.
    pub async fn delete_items(
        &self,
        authenticated_user: &user::Model,
        item_ids: &[Uuid],
    ) -> Result<(), LibraryControllerError> {
        if !authenticated_user.is_administrator {
            return Err(LibraryControllerError::Forbidden);
        }
        self.items.delete_many(item_ids).await?;
        Ok(())
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), LibraryControllerError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(LibraryControllerError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(LibraryControllerError::Forbidden);
        }
        Ok(())
    }

    fn hydrate_page(&self, mut page: BaseItemPage) -> BaseItemPage {
        page.items = page
            .items
            .into_iter()
            .filter_map(|item| self.item_types.hydrate(item))
            .map(HydratedBaseItem::into_model)
            .collect();
        page
    }
}
