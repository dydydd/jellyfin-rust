use jellyfin_data::{
    BaseItemError, BaseItemPage, BaseItemQuery, BaseItemRepository, ItemValueError,
    ItemValueRepository, OFFICIAL_ITEM_TYPE_ALIASES, PlaylistRepository, PlaylistStoreError,
    entities::{base_item, item_value, user},
};
use jellyfin_model::UserPolicy;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    HydratedBaseItem, ItemTypeRegistry, UserError, UserLibraryError, UserLibraryService,
    UserService,
};

#[derive(Debug, Error)]
pub enum LibraryControllerError {
    #[error("library item id must not be empty")]
    InvalidRequest,
    #[error("target user was not found")]
    UserNotFound,
    #[error("library item was not found")]
    ItemNotFound,
    #[error("administrator access is required")]
    Forbidden,
    #[error("user is not authorized to delete the library item")]
    Unauthorized,
    #[error("library item has no downloadable file")]
    FileNotFound,
    #[error("library item does not support downloading")]
    NotDownloadable,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error(transparent)]
    Playlist(#[from] PlaylistStoreError),
    #[error(transparent)]
    UserLibrary(#[from] UserLibraryError),
}

/// Returns the item-intrinsic download capability used by both DTO projection
/// and the download endpoint.
///
/// This mirrors the official `CanDownload()` overrides: photos are always
/// downloadable, file-backed audio and books are downloadable, and file-backed
/// videos are downloadable unless they are DVD or Blu-ray folder structures.
#[must_use]
pub fn item_can_download(item: &base_item::Model) -> bool {
    if item.item_type.eq_ignore_ascii_case("Photo") {
        return true;
    }
    if !matches!(
        item.item_type.as_str(),
        "Audio" | "AudioBook" | "Book" | "Episode" | "Movie" | "MusicVideo" | "Trailer" | "Video"
    ) || !item.path.as_deref().is_some_and(path_uses_file_protocol)
    {
        return false;
    }
    if matches!(item.item_type.as_str(), "Audio" | "AudioBook" | "Book") {
        return true;
    }
    let video_type = item
        .data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get("VideoType").or_else(|| object.get("video_type")));
    !matches!(video_type, Some(Value::String(value)) if value.eq_ignore_ascii_case("Dvd") || value.eq_ignore_ascii_case("BluRay"))
        && !matches!(video_type, Some(Value::Number(value)) if value.as_i64().is_some_and(|value| value == 2 || value == 3))
}

fn path_uses_file_protocol(path: &str) -> bool {
    if ["rtsp", "rtmp", "http", "rtp", "udp", "ftp"]
        .iter()
        .any(|prefix| {
            path.get(..prefix.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
        })
    {
        return false;
    }
    !path.contains("://")
        || path
            .get(.."file://".len())
            .is_some_and(|value| value.eq_ignore_ascii_case("file://"))
}

/// Returns the playable media path, resolving a persisted `.strm` pointer when present.
#[must_use]
pub fn media_source_path(item: &base_item::Model) -> Option<&str> {
    item.data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| {
            object
                .get("StrmTarget")
                .or_else(|| object.get("strm_target"))
        })
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .or_else(|| item.path.as_deref().filter(|path| !path.is_empty()))
}

/// Returns the item-intrinsic delete capability used before applying user policy.
///
/// This mirrors the official `CanDelete()` overrides for library items. Most
/// items require a local file-protocol path, metadata projection types are
/// never deletable, and a physical music artist is deletable independently of
/// its path. Playlist ownership is deliberately not considered here: the
/// official user-aware playlist override belongs to authorization, while the
/// user-less and batched DTO paths use this intrinsic result.
#[must_use]
pub fn item_can_delete(item: &base_item::Model) -> bool {
    let item_type = canonical_official_item_type(&item.item_type);
    if matches!(
        item_type,
        "AggregateFolder"
            | "BasePluginFolder"
            | "Channel"
            | "CollectionFolder"
            | "Genre"
            | "MusicGenre"
            | "Person"
            | "PlaylistsFolder"
            | "Program"
            | "Studio"
            | "UserRootFolder"
            | "UserView"
            | "Year"
    ) {
        return false;
    }

    if item_type == "MusicArtist" {
        return item.parent_id.is_some();
    }

    if item.is_folder && item_is_root_folder(item) {
        return false;
    }

    item.path.as_deref().is_some_and(path_uses_file_protocol)
}

fn canonical_official_item_type(item_type: &str) -> &str {
    OFFICIAL_ITEM_TYPE_ALIASES
        .iter()
        .find(|(canonical, persisted)| item_type == *canonical || item_type == *persisted)
        .map_or(item_type, |(canonical, _)| *canonical)
}

fn item_is_root_folder(item: &base_item::Model) -> bool {
    item.id == jellyfin_data::USER_ROOT_FOLDER_ID
        || item
            .data
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|object| {
                object
                    .get("IsRoot")
                    .or_else(|| object.get("isRoot"))
                    .or_else(|| object.get("is_root"))
            })
            .and_then(Value::as_bool)
            .unwrap_or_default()
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
        assert!(item_can_download(&item("Photo", None, None)));
        assert!(item_can_download(&item(
            "Audio",
            Some("file:///music.flac"),
            None
        )));
        for path in [
            "HTTP://example.test/movie.mkv",
            "rtsp://example.test/movie",
            "udp://example.test/movie",
            "ftp://example.test/movie",
            "s3://bucket/movie.flac",
        ] {
            assert!(!item_can_download(&item("Audio", Some(path), None)));
        }
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
    fn can_delete_requires_file_protocol_except_for_physical_artists() {
        assert!(item_can_delete(&item("Movie", Some("/movie.mkv"), None)));
        assert!(item_can_delete(&item(
            "Movie",
            Some("file:///movie.mkv"),
            None
        )));
        assert!(!item_can_delete(&item("Movie", None, None)));
        assert!(!item_can_delete(&item(
            "Movie",
            Some("https://example.test/movie.mkv"),
            None
        )));

        let mut virtual_item = item("Movie", Some("/virtual"), None);
        virtual_item.is_virtual_item = true;
        assert!(item_can_delete(&virtual_item));

        let mut by_name_artist = item("MusicArtist", Some("/metadata/artist"), None);
        assert!(!item_can_delete(&by_name_artist));
        by_name_artist.parent_id = Some(Uuid::new_v4());
        by_name_artist.path = None;
        assert!(item_can_delete(&by_name_artist));
    }

    #[test]
    fn can_delete_honors_folder_and_metadata_type_overrides() {
        let mut folder = item("Folder", Some("/library/folder"), None);
        folder.is_folder = true;
        assert!(item_can_delete(&folder));

        folder.data = Some(json!({"IsRoot": true}));
        assert!(!item_can_delete(&folder));

        for item_type in [
            "AggregateFolder",
            "BasePluginFolder",
            "Channel",
            "CollectionFolder",
            "Genre",
            "MusicGenre",
            "Person",
            "PlaylistsFolder",
            "Studio",
            "UserRootFolder",
            "UserView",
            "Year",
        ] {
            assert!(
                !item_can_delete(&item(item_type, Some("/metadata/item"), None)),
                "{item_type}"
            );
        }

        assert!(item_can_delete(&item(
            "MusicAlbum",
            Some("/music/album"),
            None
        )));
        assert!(item_can_delete(&item(
            "BoxSet",
            Some("/metadata/collections/set"),
            None
        )));

        // Playlist ownership/admin checks are user-context authorization. The
        // intrinsic path follows Folder/BaseItem for user-less and page DTOs.
        assert!(item_can_delete(&item(
            "Playlist",
            Some("/metadata/playlists/list"),
            None
        )));
        assert!(!item_can_delete(&item("Playlist", None, None)));
    }

    #[test]
    fn can_delete_resolves_official_clr_item_type_aliases() {
        assert!(!item_can_delete(&item(
            "MediaBrowser.Controller.Entities.Person",
            Some("/metadata/person"),
            None
        )));
        assert!(item_can_delete(&item(
            "MediaBrowser.Controller.Entities.Audio.MusicAlbum",
            Some("/music/album"),
            None
        )));

        let mut physical_artist = item(
            "MediaBrowser.Controller.Entities.Audio.MusicArtist",
            None,
            None,
        );
        physical_artist.parent_id = Some(Uuid::new_v4());
        assert!(item_can_delete(&physical_artist));
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
    user_library: UserLibraryService,
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
            user_library: UserLibraryService::with_item_type_registry(
                std::sync::Arc::clone(&database),
                item_types.clone(),
            ),
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
        let policy: UserPolicy = serde_json::from_value(authenticated_user.policy.clone())
            .map_err(UserError::PolicySerialization)?;
        if !policy.enable_content_downloading {
            return Err(LibraryControllerError::Forbidden);
        }
        let item = self
            .user_library
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        if !item_can_download(&item) {
            return Err(LibraryControllerError::NotDownloadable);
        }
        item.path.ok_or(LibraryControllerError::FileNotFound)
    }

    /// Resolves a globally visible download path for API-key authentication.
    ///
    /// # Errors
    ///
    /// Returns not-found or unsupported-download errors.
    pub async fn download_path_without_user(
        &self,
        item_id: Uuid,
    ) -> Result<String, LibraryControllerError> {
        let item = self.item_without_user(item_id).await?;
        if !item_can_download(&item) {
            return Err(LibraryControllerError::NotDownloadable);
        }
        item.path.ok_or(LibraryControllerError::FileNotFound)
    }

    /// Resolves the original item path without applying download permissions.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, missing-file, or persistence errors.
    pub async fn file_path(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<String, LibraryControllerError> {
        self.user_library
            .item(authenticated_user, target_user_id, item_id)
            .await?
            .path
            .ok_or(LibraryControllerError::FileNotFound)
    }

    /// Resolves the original item path without a user context.
    ///
    /// # Errors
    ///
    /// Returns not-found, missing-file, or persistence errors.
    pub async fn file_path_without_user(
        &self,
        item_id: Uuid,
    ) -> Result<String, LibraryControllerError> {
        self.item_without_user(item_id)
            .await?
            .path
            .ok_or(LibraryControllerError::FileNotFound)
    }

    async fn item_without_user(
        &self,
        item_id: Uuid,
    ) -> Result<base_item::Model, LibraryControllerError> {
        self.items
            .get(item_id)
            .await?
            .and_then(|item| self.item_types.hydrate(item))
            .map(HydratedBaseItem::into_model)
            .ok_or(LibraryControllerError::ItemNotFound)
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
        exclude_artist_ids: &[Uuid],
        limit: Option<i32>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        let item = self
            .user_library
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        if item_has_empty_similar_result(&item) || limit.is_some_and(|limit| limit <= 0) {
            return Ok(BaseItemPage {
                items: Vec::new(),
                total_record_count: 0,
                start_index: 0,
            });
        }
        let media_types = item.media_type.into_iter().collect();
        let mut query = BaseItemQuery {
            exclude_ids: vec![item.id],
            exclude_artist_ids: exclude_artist_ids.to_vec(),
            include_item_types: vec![item.item_type],
            media_types,
            is_virtual_item: Some(false),
            limit: Some(limit.map_or(50, |limit| u64::try_from(limit).unwrap_or_default())),
            enable_total_record_count: Some(false),
            ..Default::default()
        };
        self.user_library
            .apply_base_item_policy(authenticated_user, target_user_id, &mut query)
            .await?;
        let page = self.items.query(&query).await?;
        let mut page = self.hydrate_page(page);
        page.total_record_count = u64::try_from(page.items.len()).unwrap_or(u64::MAX);
        Ok(page)
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
        limit: Option<i32>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.instant_mix_for_item(authenticated_user, target_user_id, item_id, limit, false)
            .await
    }

    /// Creates an instant mix only when the seed is a playlist.
    ///
    /// # Errors
    ///
    /// Returns not-found when the seed is not a playlist, or normal instant-mix errors.
    pub async fn instant_mix_for_playlist(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        limit: Option<i32>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.instant_mix_for_item(authenticated_user, target_user_id, item_id, limit, true)
            .await
    }

    async fn instant_mix_for_item(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        limit: Option<i32>,
        require_playlist: bool,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        let item = self
            .user_library
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        if require_playlist && item.item_type != "Playlist" {
            return Err(LibraryControllerError::ItemNotFound);
        }
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
        let mut access_policy = BaseItemQuery::default();
        self.user_library
            .apply_base_item_policy(authenticated_user, target_user_id, &mut access_policy)
            .await?;
        let seed_is_audio = item.item_type == "Audio";
        let genre_ids = if item.item_type == "MusicGenre" {
            vec![item.id]
        } else if matches!(
            item.item_type.as_str(),
            "Playlist" | "MusicAlbum" | "MusicArtist" | "Audio"
        ) {
            self.item_values
                .values_for_item(item_id, item_value::ItemValueType::Genre)
                .await?
                .into_iter()
                .map(|genre| genre.item_value_id)
                .collect()
        } else if item.is_folder {
            self.item_values
                .genre_ids_for_folder_audio(item.id, &access_policy)
                .await?
        } else {
            return Ok(empty_item_page());
        };
        let mut items = self
            .item_values
            .random_audio_for_genres(&genre_ids, 201, &access_policy)
            .await?;
        items.retain(|candidate| candidate.id != item.id);
        items.truncate(200_usize.saturating_sub(usize::from(seed_is_audio)));
        if seed_is_audio {
            items.insert(0, item);
        }
        Ok(self.finish_instant_mix(items, limit))
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
        limit: Option<i32>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.item_values
            .get_by_id(genre_id, item_value::ItemValueType::Genre)
            .await?
            .ok_or(LibraryControllerError::ItemNotFound)?;
        let mut access_policy = BaseItemQuery::default();
        self.user_library
            .apply_base_item_policy(authenticated_user, target_user_id, &mut access_policy)
            .await?;
        let items = self
            .item_values
            .random_audio_for_genres(&[genre_id], 200, &access_policy)
            .await?;
        Ok(self.finish_instant_mix(items, limit))
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
        limit: Option<i32>,
    ) -> Result<BaseItemPage, LibraryControllerError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let Some(genre_id) = self
            .item_values
            .get_normalized(item_value::ItemValueType::Genre, genre_name)
            .await?
            .map(|genre| genre.item_value_id)
        else {
            return Ok(empty_item_page());
        };
        let mut access_policy = BaseItemQuery::default();
        self.user_library
            .apply_base_item_policy(authenticated_user, target_user_id, &mut access_policy)
            .await?;
        let items = self
            .item_values
            .random_audio_for_genres(&[genre_id], 200, &access_policy)
            .await?;
        Ok(self.finish_instant_mix(items, limit))
    }

    fn finish_instant_mix(
        &self,
        mut items: Vec<base_item::Model>,
        limit: Option<i32>,
    ) -> BaseItemPage {
        let total_record_count = u64::try_from(items.len()).unwrap_or(u64::MAX);
        if let Some(limit) = limit {
            items.truncate(usize::try_from(limit).unwrap_or_default().min(items.len()));
        }
        BaseItemPage {
            items: items
                .into_iter()
                .filter_map(|item| self.item_types.hydrate(item))
                .map(HydratedBaseItem::into_model)
                .collect(),
            total_record_count,
            start_index: 0,
        }
    }

    /// Deletes complete item subtrees and their source files in request order.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-found, protected-item, or persistence errors.
    pub async fn delete_items(
        &self,
        authenticated_user: Option<&user::Model>,
        item_ids: &[Uuid],
    ) -> Result<(), LibraryControllerError> {
        for &item_id in item_ids {
            self.delete_item(authenticated_user, item_id).await?;
        }

        Ok(())
    }

    async fn delete_item(
        &self,
        authenticated_user: Option<&user::Model>,
        item_id: Uuid,
    ) -> Result<(), LibraryControllerError> {
        // Official LibraryManager.GetItemById rejects Guid.Empty before
        // either the user-aware visibility lookup or the direct lookup.
        if item_id.is_nil() {
            return Err(LibraryControllerError::InvalidRequest);
        }

        let item = if let Some(user) = authenticated_user {
            self.user_library.item(user, user.id, item_id).await?
        } else {
            self.items
                .get(item_id)
                .await?
                .ok_or(LibraryControllerError::ItemNotFound)?
        };

        if let Some(user) = authenticated_user
            && !self.item_can_delete_for_user(&item, user).await?
        {
            return Err(LibraryControllerError::Unauthorized);
        }

        let mut paths = item
            .path
            .filter(|path| !path.is_empty())
            .into_iter()
            .collect::<Vec<_>>();
        for descendant in self.items.descendants(item_id).await? {
            if let Some(path) = descendant.item.path.filter(|path| !path.is_empty()) {
                paths.push(path);
            }
        }

        self.items.delete_many(&[item_id]).await?;
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

    async fn item_can_delete_for_user(
        &self,
        item: &base_item::Model,
        user: &user::Model,
    ) -> Result<bool, LibraryControllerError> {
        let item_type = canonical_official_item_type(&item.item_type);
        if item_type == "Playlist" {
            let playlist = self
                .playlists
                .get(item.id)
                .await?
                .ok_or(LibraryControllerError::ItemNotFound)?;
            return Ok(user.is_administrator || playlist.owner_user_id == Some(user.id));
        }
        if !item_can_delete(item) {
            return Ok(false);
        }

        let policy: UserPolicy =
            serde_json::from_value(user.policy.clone()).map_err(UserError::PolicySerialization)?;
        if item_type == "BoxSet" {
            return Ok(user.is_administrator || policy.enable_collection_management);
        }
        Ok(
            policy.enable_content_deletion
                || self.deletion_folder_allowed(item.id, &policy).await?,
        )
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
            .collect::<Vec<_>>();
        if allowed.is_empty() {
            return Ok(false);
        }
        Ok(self
            .items
            .item_ids_in_collection_folders(&[item_id], &allowed)
            .await?
            .contains(&item_id))
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

fn item_has_empty_similar_result(item: &base_item::Model) -> bool {
    matches!(
        item.item_type.as_str(),
        "Episode" | "Genre" | "MusicGenre" | "Person" | "Studio" | "Year"
    )
}

fn empty_item_page() -> BaseItemPage {
    BaseItemPage {
        items: Vec::new(),
        total_record_count: 0,
        start_index: 0,
    }
}
