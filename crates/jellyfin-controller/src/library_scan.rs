use std::path::Path;

use jellyfin_data::{
    BaseItemError, BaseItemRepository, NewBaseItem, USER_ROOT_FOLDER_ID, VirtualFolderError,
    VirtualFolderRepository,
};
use md5::{Digest, Md5};
use sea_orm::DatabaseConnection;
use serde_json::json;
use thiserror::Error;
use tokio::fs;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryScanSummary {
    pub folders_seen: usize,
    pub items_added: usize,
    pub items_seen: usize,
}

#[derive(Debug, Error)]
pub enum LibraryScanError {
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct LibraryScanService {
    folders: VirtualFolderRepository,
    items: BaseItemRepository,
}

impl LibraryScanService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            folders: VirtualFolderRepository::new(database.clone()),
            items: BaseItemRepository::new(database),
        }
    }

    /// Scans configured virtual-folder paths into directly playable base items.
    ///
    /// # Errors
    ///
    /// Returns persistence or file-system errors that prevent the scan from
    /// reading configured paths or writing discovered media.
    pub async fn scan_all(&self) -> Result<LibraryScanSummary, LibraryScanError> {
        self.items.ensure_user_root().await?;
        let mut summary = LibraryScanSummary::default();
        for folder in self.folders.list().await? {
            let collection = self.ensure_collection_folder(&folder).await?;
            summary.folders_seen += 1;
            for path in folder.paths {
                self.scan_path(
                    Path::new(&path.normalized_path),
                    collection.id,
                    &mut summary,
                )
                .await?;
            }
        }
        Ok(summary)
    }

    async fn ensure_collection_folder(
        &self,
        folder: &jellyfin_data::VirtualFolderWithPaths,
    ) -> Result<jellyfin_data::entities::base_item::Model, LibraryScanError> {
        if let Some(mut item) = self.items.get(folder.folder.id).await? {
            item.item_type = "CollectionFolder".to_owned();
            item.parent_id = Some(USER_ROOT_FOLDER_ID);
            item.name = Some(folder.folder.name.clone());
            item.sort_name = Some(folder.folder.name.clone());
            item.media_type = None;
            item.is_folder = true;
            item.is_virtual_item = false;
            item.data = Some(json!({
                "CollectionType": folder.folder.collection_type,
                "LibraryOptions": folder.folder.library_options,
            }));
            return Ok(self.items.update(item).await?);
        }

        let mut item = NewBaseItem::new(folder.folder.id, "CollectionFolder");
        item.parent_id = Some(USER_ROOT_FOLDER_ID);
        item.name = Some(folder.folder.name.clone());
        item.sort_name = item.name.clone();
        item.is_folder = true;
        item.data = Some(json!({
            "CollectionType": folder.folder.collection_type,
            "LibraryOptions": folder.folder.library_options,
        }));
        Ok(self.items.create(item).await?)
    }

    async fn scan_path(
        &self,
        root: &Path,
        parent_id: Uuid,
        summary: &mut LibraryScanSummary,
    ) -> Result<(), LibraryScanError> {
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let mut entries = match fs::read_dir(&directory).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            while let Some(entry) = entries.next_entry().await? {
                let metadata = entry.metadata().await?;
                let path = entry.path();
                if metadata.is_dir() {
                    if !is_ignored_directory(&path) {
                        pending.push(path);
                    }
                    continue;
                }
                if !metadata.is_file() || is_ignored_file(&path) {
                    continue;
                }
                let Some(media_kind) = media_kind(&path) else {
                    continue;
                };
                summary.items_seen += 1;
                if self.ensure_media_item(&path, parent_id, media_kind).await? {
                    summary.items_added += 1;
                }
            }
        }
        Ok(())
    }

    async fn ensure_media_item(
        &self,
        path: &Path,
        parent_id: Uuid,
        media_kind: MediaKind,
    ) -> Result<bool, LibraryScanError> {
        let Some(path) = path.to_str() else {
            return Ok(false);
        };
        if self.items.exists_by_path(path).await? {
            return Ok(false);
        }

        let name = display_name(path);
        let mut item = NewBaseItem::new(stable_item_id(path), media_kind.item_type());
        item.path = Some(path.to_owned());
        item.parent_id = Some(parent_id);
        item.name = Some(name.clone());
        item.sort_name = Some(name);
        item.media_type = Some(media_kind.media_type().to_owned());
        item.is_folder = false;
        item.is_virtual_item = false;
        item.presentation_unique_key = Some(path.to_owned());
        item.data = Some(json!({
            "Container": Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase),
        }));
        self.items.create(item).await?;
        Ok(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Audio,
    Video,
}

impl MediaKind {
    const fn item_type(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Video => "Video",
        }
    }

    const fn media_type(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Video => "Video",
        }
    }
}

fn media_kind(path: &Path) -> Option<MediaKind> {
    let extension = path.extension()?.to_str()?;
    if AUDIO_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
    {
        return Some(MediaKind::Audio);
    }
    VIDEO_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
        .then_some(MediaKind::Video)
}

fn display_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .or_else(|| Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn stable_item_id(path: &str) -> Uuid {
    let digest = Md5::digest(path.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest);
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn is_ignored_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                ".git" | ".svn" | "@eadir" | "metadata" | "extrafanart" | "extrathumbs"
            )
        })
}

fn is_ignored_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "ape", "dsf", "flac", "m4a", "m4b", "mp3", "ogg", "opus", "wav", "wma",
];

const VIDEO_EXTENSIONS: &[&str] = &[
    "avi", "divx", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "ts", "webm",
    "wmv",
];

#[cfg(test)]
mod tests {
    use super::{MediaKind, display_name, media_kind, stable_item_id};
    use std::path::Path;

    #[test]
    fn media_kind_accepts_common_direct_play_extensions() {
        assert_eq!(media_kind(Path::new("movie.MKV")), Some(MediaKind::Video));
        assert_eq!(media_kind(Path::new("song.FlAc")), Some(MediaKind::Audio));
        assert_eq!(media_kind(Path::new("poster.jpg")), None);
    }

    #[test]
    fn stable_ids_are_path_specific_and_repeatable() {
        assert_eq!(
            stable_item_id("/media/movie.mkv"),
            stable_item_id("/media/movie.mkv")
        );
        assert_ne!(
            stable_item_id("/media/a.mkv"),
            stable_item_id("/media/b.mkv")
        );
    }

    #[test]
    fn display_name_uses_file_stem() {
        assert_eq!(display_name("/media/Movie Name.mkv"), "Movie Name");
    }
}
