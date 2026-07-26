use std::path::Path;

use jellyfin_data::{
    BaseItemError, BaseItemRepository, MediaStreamQuery, MediaStreamRepository,
    MediaStreamStoreError, NewBaseItem, PersistedMediaStream, PersistedMediaStreamType,
    USER_ROOT_FOLDER_ID, VirtualFolderError, VirtualFolderRepository,
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
    MediaStream(#[from] MediaStreamStoreError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct LibraryScanService {
    folders: VirtualFolderRepository,
    items: BaseItemRepository,
    streams: MediaStreamRepository,
}

impl LibraryScanService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            folders: VirtualFolderRepository::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            streams: MediaStreamRepository::new(database),
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
        if let Some(existing) = self.items.by_paths(&[path.to_owned()]).await?.pop() {
            self.ensure_default_streams(existing.id, path, media_kind)
                .await?;
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
        let item = self.items.create(item).await?;
        self.ensure_default_streams(item.id, path, media_kind)
            .await?;
        Ok(true)
    }

    async fn ensure_default_streams(
        &self,
        item_id: Uuid,
        path: &str,
        media_kind: MediaKind,
    ) -> Result<(), LibraryScanError> {
        let existing = self
            .streams
            .query(MediaStreamQuery {
                item_id,
                stream_index: None,
                stream_type: None,
            })
            .await?;
        if !existing.is_empty() {
            return Ok(());
        }
        self.streams
            .replace(item_id, &[default_stream(path, media_kind)])
            .await?;
        Ok(())
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

fn default_stream(path: &str, media_kind: MediaKind) -> PersistedMediaStream {
    PersistedMediaStream {
        stream_index: 0,
        stream_type: match media_kind {
            MediaKind::Audio => PersistedMediaStreamType::Audio,
            MediaKind::Video => PersistedMediaStreamType::Video,
        },
        codec: codec_from_extension(path),
        language: None,
        channel_layout: (media_kind == MediaKind::Audio).then(|| "stereo".to_owned()),
        profile: None,
        aspect_ratio: None,
        path: None,
        is_interlaced: Some(false),
        bit_rate: None,
        channels: (media_kind == MediaKind::Audio).then_some(2),
        sample_rate: (media_kind == MediaKind::Audio).then_some(48_000),
        is_default: true,
        is_forced: false,
        is_external: false,
        is_original: false,
        height: None,
        width: None,
        average_frame_rate: None,
        real_frame_rate: None,
        level: None,
        pixel_format: None,
        bit_depth: None,
        is_anamorphic: None,
        ref_frames: None,
        codec_tag: None,
        comment: None,
        nal_length_size: None,
        is_avc: None,
        title: None,
        time_base: None,
        codec_time_base: None,
        color_primaries: None,
        color_space: None,
        color_transfer: None,
        dv_version_major: None,
        dv_version_minor: None,
        dv_profile: None,
        dv_level: None,
        rpu_present_flag: None,
        el_present_flag: None,
        bl_present_flag: None,
        dv_bl_signal_compatibility_id: None,
        is_hearing_impaired: Some(false),
        rotation: None,
        hdr10_plus_present_flag: None,
    }
}

fn codec_from_extension(path: &str) -> Option<String> {
    let extension = Path::new(path).extension()?.to_str()?;
    let codec = if extension.eq_ignore_ascii_case("mp3") {
        "mp3"
    } else if extension.eq_ignore_ascii_case("flac") {
        "flac"
    } else if extension.eq_ignore_ascii_case("aac") {
        "aac"
    } else if extension.eq_ignore_ascii_case("opus") {
        "opus"
    } else if extension.eq_ignore_ascii_case("wav") {
        "pcm_s16le"
    } else if extension.eq_ignore_ascii_case("ogg") {
        "vorbis"
    } else if extension.eq_ignore_ascii_case("wma") {
        "wmav2"
    } else if ["mp4", "m4v", "mov"]
        .iter()
        .any(|value| extension.eq_ignore_ascii_case(value))
    {
        "h264"
    } else if extension.eq_ignore_ascii_case("webm") {
        "vp9"
    } else if extension.eq_ignore_ascii_case("wmv") {
        "wmv3"
    } else {
        return None;
    };
    Some(codec.to_owned())
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
    use super::{
        MediaKind, codec_from_extension, default_stream, display_name, media_kind, stable_item_id,
    };
    use jellyfin_data::PersistedMediaStreamType;
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

    #[test]
    fn default_streams_are_playback_visible_without_probe_data() {
        let video = default_stream("/media/Movie.mp4", MediaKind::Video);
        assert_eq!(video.stream_type, PersistedMediaStreamType::Video);
        assert_eq!(video.codec.as_deref(), Some("h264"));
        assert!(video.is_default);

        let audio = default_stream("/media/Song.flac", MediaKind::Audio);
        assert_eq!(audio.stream_type, PersistedMediaStreamType::Audio);
        assert_eq!(audio.codec.as_deref(), Some("flac"));
        assert_eq!(audio.channels, Some(2));
        assert_eq!(audio.sample_rate, Some(48_000));
    }

    #[test]
    fn codec_inference_is_conservative_for_container_only_formats() {
        assert_eq!(codec_from_extension("/media/Movie.mkv"), None);
        assert_eq!(
            codec_from_extension("/media/Clip.webm").as_deref(),
            Some("vp9")
        );
    }
}
