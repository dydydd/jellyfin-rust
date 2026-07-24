use jellyfin_data::{
    BaseItemError, BaseItemRepository, LinkedChildRepository, LinkedChildStoreError,
    PlaylistRecord, PlaylistRepository, PlaylistStoreError, PlaylistUserPermission,
    entities::base_item,
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PlaylistError {
    #[error("playlist name cannot be blank")]
    InvalidName,
    #[error("playlist was not found")]
    NotFound,
    #[error("playlist access is forbidden")]
    Forbidden,
    #[error(transparent)]
    Store(#[from] PlaylistStoreError),
    #[error(transparent)]
    Links(#[from] LinkedChildStoreError),
    #[error(transparent)]
    Items(#[from] BaseItemError),
}

#[derive(Clone)]
pub struct PlaylistService {
    items: BaseItemRepository,
    playlists: PlaylistRepository,
    links: LinkedChildRepository,
}

#[derive(Clone, Debug)]
pub struct PlaylistItem {
    pub entry_id: Uuid,
    pub item: base_item::Model,
}

#[derive(Clone, Debug)]
pub struct PlaylistItemPage {
    pub items: Vec<PlaylistItem>,
    pub total_record_count: usize,
    pub start_index: usize,
}

impl PlaylistService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            items: BaseItemRepository::new(database.clone()),
            playlists: PlaylistRepository::new(database.clone()),
            links: LinkedChildRepository::new(database),
        }
    }

    /// Creates a playlist owned by the effective user.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors.
    pub async fn create(
        &self,
        name: String,
        owner_user_id: Uuid,
        open_access: bool,
        media_type: Option<String>,
        shares: &[PlaylistUserPermission],
        item_ids: &[Uuid],
    ) -> Result<Uuid, PlaylistError> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(PlaylistError::InvalidName);
        }
        let root_id = self.items.ensure_user_root().await?.id;
        let playlist_id = Uuid::new_v4();
        self.playlists
            .create(
                playlist_id,
                name,
                root_id,
                owner_user_id,
                open_access,
                normalize_media_type(media_type),
                shares,
                item_ids,
            )
            .await?;
        Ok(playlist_id)
    }

    /// Loads a playlist visible to a user.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence errors.
    pub async fn get_for_user(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
    ) -> Result<PlaylistRecord, PlaylistError> {
        let playlist = self
            .playlists
            .get(playlist_id)
            .await?
            .ok_or(PlaylistError::NotFound)?;
        if can_read(&playlist, user_id) {
            Ok(playlist)
        } else {
            Err(PlaylistError::NotFound)
        }
    }

    /// Lists ordered item identifiers visible through one playlist.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn item_ids(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<Uuid>, PlaylistError> {
        let playlist = self.get_for_user(playlist_id, user_id).await?;
        if !can_read(&playlist, user_id) {
            return Err(PlaylistError::Forbidden);
        }
        Ok(self
            .links
            .list(playlist_id)
            .await?
            .into_iter()
            .map(|link| link.child_id)
            .collect())
    }

    /// Loads a page of ordered playlist entries and their library models.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn items(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
        start_index: usize,
        limit: Option<usize>,
    ) -> Result<PlaylistItemPage, PlaylistError> {
        self.get_for_user(playlist_id, user_id).await?;
        let links = self.links.list(playlist_id).await?;
        let total_record_count = links.len();
        let page_links = links
            .into_iter()
            .skip(start_index)
            .take(limit.unwrap_or(usize::MAX))
            .collect::<Vec<_>>();
        let ids = page_links
            .iter()
            .map(|link| link.child_id)
            .collect::<Vec<_>>();
        let models = self
            .items
            .get_many(&ids)
            .await?
            .into_iter()
            .map(|item| (item.id, item))
            .collect::<std::collections::HashMap<_, _>>();
        let items = page_links
            .into_iter()
            .filter_map(|link| {
                models
                    .get(&link.child_id)
                    .cloned()
                    .map(|item| PlaylistItem {
                        entry_id: link.child_id,
                        item,
                    })
            })
            .collect();
        Ok(PlaylistItemPage {
            items,
            total_record_count,
            start_index,
        })
    }

    /// Appends new unique items for an owner or editable share.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn add_items(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
        item_ids: &[Uuid],
        position: Option<i32>,
    ) -> Result<(), PlaylistError> {
        let playlist = self.get_for_user(playlist_id, user_id).await?;
        if !can_edit(&playlist, user_id) {
            return Err(PlaylistError::Forbidden);
        }
        self.links
            .add_manual_at(playlist_id, item_ids, position)
            .await?;
        Ok(())
    }

    /// Moves an entry for an owner or editable share.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn move_item(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
        entry_id: Uuid,
        new_index: usize,
    ) -> Result<(), PlaylistError> {
        let playlist = self.get_for_user(playlist_id, user_id).await?;
        if !can_edit(&playlist, user_id) {
            return Err(PlaylistError::Forbidden);
        }
        self.links.move_to(playlist_id, entry_id, new_index).await?;
        Ok(())
    }

    /// Removes matching entries for an owner or editable share.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn remove_items(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
        item_ids: &[Uuid],
    ) -> Result<(), PlaylistError> {
        let playlist = self.get_for_user(playlist_id, user_id).await?;
        if !can_edit(&playlist, user_id) {
            return Err(PlaylistError::Forbidden);
        }
        self.links.remove_compact(playlist_id, item_ids).await?;
        Ok(())
    }

    /// Updates optional playlist metadata and optionally replaces all items.
    ///
    /// # Errors
    ///
    /// Returns validation, not-found, forbidden, or persistence errors.
    pub async fn update(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
        name: Option<String>,
        item_ids: Option<&[Uuid]>,
        shares: Option<&[PlaylistUserPermission]>,
        open_access: Option<bool>,
    ) -> Result<(), PlaylistError> {
        let playlist = self.get_for_user(playlist_id, user_id).await?;
        if !can_edit(&playlist, user_id) {
            return Err(PlaylistError::Forbidden);
        }
        let name = name
            .map(|name| name.trim().to_owned())
            .map(|name| {
                if name.is_empty() {
                    Err(PlaylistError::InvalidName)
                } else {
                    Ok(name)
                }
            })
            .transpose()?;
        self.playlists
            .update(playlist_id, name, open_access, shares, item_ids)
            .await?;
        Ok(())
    }

    /// Lists shares for the playlist owner.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn users(
        &self,
        playlist_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<PlaylistUserPermission>, PlaylistError> {
        let playlist = self.get_for_user(playlist_id, user_id).await?;
        if playlist.owner_user_id != Some(user_id) {
            return Err(PlaylistError::Forbidden);
        }
        Ok(playlist.shares)
    }

    /// Resolves one permission visible to its subject or an editor.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn user(
        &self,
        playlist_id: Uuid,
        calling_user_id: Uuid,
        target_user_id: Uuid,
    ) -> Result<PlaylistUserPermission, PlaylistError> {
        let playlist = self.get_for_user(playlist_id, calling_user_id).await?;
        if playlist.owner_user_id == Some(calling_user_id) {
            return Ok(PlaylistUserPermission {
                user_id: calling_user_id,
                can_edit: true,
            });
        }
        if calling_user_id != target_user_id && !can_edit(&playlist, calling_user_id) {
            return Err(PlaylistError::Forbidden);
        }
        playlist
            .shares
            .into_iter()
            .find(|share| share.user_id == target_user_id)
            .ok_or(PlaylistError::NotFound)
    }

    /// Adds or replaces one share as the playlist owner.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn set_user(
        &self,
        playlist_id: Uuid,
        calling_user_id: Uuid,
        target_user_id: Uuid,
        can_edit: bool,
    ) -> Result<(), PlaylistError> {
        let playlist = self.get_for_user(playlist_id, calling_user_id).await?;
        if playlist.owner_user_id != Some(calling_user_id) {
            return Err(PlaylistError::Forbidden);
        }
        let mut shares = playlist.shares;
        shares.retain(|share| share.user_id != target_user_id);
        shares.push(PlaylistUserPermission {
            user_id: target_user_id,
            can_edit,
        });
        self.playlists
            .update(playlist_id, None, None, Some(&shares), None)
            .await?;
        Ok(())
    }

    /// Removes one share as an owner or editable share.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn remove_user(
        &self,
        playlist_id: Uuid,
        calling_user_id: Uuid,
        target_user_id: Uuid,
    ) -> Result<(), PlaylistError> {
        let playlist = self.get_for_user(playlist_id, calling_user_id).await?;
        if !can_edit(&playlist, calling_user_id) {
            return Err(PlaylistError::Forbidden);
        }
        if !playlist
            .shares
            .iter()
            .any(|share| share.user_id == target_user_id)
        {
            return Err(PlaylistError::NotFound);
        }
        let shares = playlist
            .shares
            .into_iter()
            .filter(|share| share.user_id != target_user_id)
            .collect::<Vec<_>>();
        self.playlists
            .update(playlist_id, None, None, Some(&shares), None)
            .await?;
        Ok(())
    }
}

fn can_read(playlist: &PlaylistRecord, user_id: Uuid) -> bool {
    playlist.open_access
        || playlist.owner_user_id == Some(user_id)
        || playlist.shares.iter().any(|share| share.user_id == user_id)
}

fn can_edit(playlist: &PlaylistRecord, user_id: Uuid) -> bool {
    playlist.owner_user_id == Some(user_id)
        || playlist
            .shares
            .iter()
            .any(|share| share.user_id == user_id && share.can_edit)
}

fn normalize_media_type(media_type: Option<String>) -> Option<String> {
    media_type
        .map(|media_type| media_type.trim().to_owned())
        .filter(|media_type| !media_type.is_empty() && !media_type.eq_ignore_ascii_case("Unknown"))
        .or_else(|| Some("Audio".to_owned()))
}
