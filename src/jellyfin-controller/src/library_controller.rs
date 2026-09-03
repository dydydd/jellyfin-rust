use jellyfin_data::{
    BaseItemCounts, BaseItemError, BaseItemPage, BaseItemQuery, BaseItemRepository, ItemValueError,
    ItemValueRepository, PlaylistRepository, PlaylistStoreError,
    entities::{base_item, item_value, user},
};
use jellyfin_model::UserPolicy;
use serde_json::Value;
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

fn item_can_download(item: &base_item::Model) -> bool {
    if item.is_folder || item.is_virtual_item || item.path.as_deref().is_none_or(str::is_empty) {
        return false;
    }
    if !matches!(
        item.item_type.as_str(),
        "Audio"
            | "AudioBook"
            | "Book"
            | "Episode"
            | "Movie"
            | "MusicVideo"
            | "Photo"
            | "Trailer"
            | "Video"
    ) {
        return false;
    }
    let video_type = item
        .data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get("VideoType").or_else(|| object.get("video_type")));
    !matches!(video_type, Some(Value::String(value)) if value.eq_ignore_ascii_case("Dvd") || value.eq_ignore_ascii_case("BluRay"))
        && !matches!(video_type, Some(Value::Number(value)) if value.as_i64().is_some_and(|value| value == 2 || value == 3))
}

/// Mirrors the conservative part of `BaseItem.CanDelete()`: virtual and
/// aggregate metadata entries are projections, not user-owned files.  Only
/// concrete file/folder items (with a real local path) may reach the delete
/// repository operation.
fn item_can_delete(item: &base_item::Model) -> bool {
    if item.id == jellyfin_data::USER_ROOT_FOLDER_ID
        || item.is_virtual_item
        || item.path.as_deref().is_none_or(str::is_empty)
    {
        return false;
    }
    !matches!(
        item.item_type.as_str(),
        "UserView"
            | "CollectionFolder"
            | "AggregateFolder"
            | "UserRootFolder"
            | "Person"
            | "Genre"
            | "Studio"
            | "MusicArtist"
            | "MusicAlbum"
            | "BoxSet"
            | "Playlist"
            | "Channel"
            | "Program"
    )
}

#[cfg(test)]
mod tests {
    use super::{item_can_delete, item_can_download};
    use jellyfin_data::entities::base_item;
    use serde_json::json;
    use uuid::Uuid;

    fn item(
        item_type: &str,
        path: Option<&str>,
        data: Option<serde_json::Value>,
    ) -> base_item::Model {
        let now = chrono::Utc::now();
        base_item::Model {
            id: Uuid::new_v4(),
            item_type: item_type.to_owned(),
            data,
            path: path.map(str::to_owned),
            parent_id: None,
            top_parent_id: None,
            name: None,
            clean_name: None,
            sort_name: None,
            media_type: None,
            overview: None,
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: now,
            date_modified: now,
            row_version: 0,
        }
    }

    #[test]
    fn can_download_matches_file_backed_media_and_rejects_discs() {
        assert!(item_can_download(&item("Movie", Some("/movie.mkv"), None)));
        assert!(!item_can_download(&item("Movie", None, None)));
        assert!(!item_can_download(&item("Folder", Some("/folder"), None)));
        assert!(!item_can_download(&item(
            "Movie",
            Some("/disc"),
            Some(json!({"VideoType": "Dvd"}))
        )));
        assert!(!item_can_download(&item(
            "Movie",
            Some("/disc"),
            Some(json!({"VideoType": 3}))
        )));
    }

    #[test]
    fn can_delete_rejects_virtual_and_aggregate_items() {
        assert!(item_can_delete(&item("Movie", Some("/movie.mkv"), None)));
        assert!(!item_can_delete(&item("UserView", Some("/view"), None)));
        assert!(!item_can_delete(&item(
            "CollectionFolder",
            Some("/library"),
            None
        )));
        let mut virtual_item = item("Movie", Some("/virtual"), None);
        virtual_item.is_virtual_item = true;
        assert!(!item_can_delete(&virtual_item));
    }
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
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self::with_item_type_registry(database, ItemTypeRegistry::default())
    }

    #[must_use]
    pub fn with_item_type_registry(
        database: impl Into<jellyfin_data::SharedDatabase>,
        item_types: ItemTypeRegistry,
    ) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            item_values: ItemValueRepository::new(std::sync::Arc::clone(&database)),
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
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        let policy: UserPolicy = serde_json::from_value(authenticated_user.policy.clone())
            .map_err(UserError::PolicySerialization)?;
        if !policy.enable_content_downloading || !item_can_download(&item) {
            return Err(LibraryControllerError::Forbidden);
        }
        item.path
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
    /// `PostgreSQL` from shared genre mappings up to Jellyfin's 200-item cap.
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

    /// Atomically deletes complete item subtrees and their source files.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-found, protected-item, or persistence errors.
    pub async fn delete_items(
        &self,
        authenticated_user: &user::Model,
        item_ids: &[Uuid],
    ) -> Result<(), LibraryControllerError> {
        let policy: UserPolicy = serde_json::from_value(authenticated_user.policy.clone())
            .map_err(UserError::PolicySerialization)?;
        let mut paths = Vec::new();
        for &item_id in item_ids {
            let item = self
                .items
                .get(item_id)
                .await?
                .ok_or(LibraryControllerError::ItemNotFound)?;
            if !item_can_delete(&item) {
                return Err(LibraryControllerError::Forbidden);
            }
            if !authenticated_user.is_administrator
                && !self.deletion_folder_allowed(item_id, &policy).await?
            {
                return Err(LibraryControllerError::Forbidden);
            }
            if let Some(path) = item.path.filter(|path| !path.is_empty()) {
                paths.push(path);
            }
            for descendant in self.items.descendants(item_id).await? {
                if let Some(path) = descendant.item.path.filter(|path| !path.is_empty()) {
                    paths.push(path);
                }
            }
        }
        self.items.delete_many(item_ids).await?;
        for path in paths {
            let path = std::path::Path::new(&path);
            let result = if path.is_dir() {
                tokio::fs::remove_dir_all(path).await
            } else {
                tokio::fs::remove_file(path).await
            };
            if let Err(error) = result
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), %error, "failed to remove deleted library item file");
            }
        }
        Ok(())
    }

    async fn deletion_folder_allowed(
        &self,
        item_id: Uuid,
        policy: &UserPolicy,
    ) -> Result<bool, LibraryControllerError> {
        if policy.enable_content_deletion {
            return Ok(item_id != jellyfin_data::USER_ROOT_FOLDER_ID);
        }
        let allowed = policy
            .enable_content_deletion_from_folders
            .iter()
            .filter_map(|id| Uuid::parse_str(id).ok())
            .collect::<std::collections::HashSet<_>>();
        if allowed.is_empty() {
            return Ok(false);
        }
        if allowed.contains(&item_id) {
            return Ok(true);
        }
        Ok(self
            .items
            .ancestors(item_id)
            .await?
            .into_iter()
            .any(|entry| allowed.contains(&entry.item.id)))
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
