use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, MediaStreamQuery, MediaStreamRepository,
    MediaStreamStoreError, NewBaseItem, PersistedMediaStream, PersistedMediaStreamType,
    USER_ROOT_FOLDER_ID, VirtualFolderError, VirtualFolderRepository,
};
use jellyfin_media_encoding::probing::{
    CommandProbeProcessRunner, ExternalMediaSource, ExternalProbeOptions, ExternalSourceProber,
    MediaInfo, MediaProtocol, MediaStream, MediaStreamType,
};
use md5::{Digest, Md5};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::fs;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryScanSummary {
    pub folders_seen: usize,
    pub items_added: usize,
    pub items_removed: usize,
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
    probe_path: PathBuf,
}

impl LibraryScanService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self::with_probe_path(database, "ffprobe")
    }

    #[must_use]
    pub fn with_probe_path(database: DatabaseConnection, probe_path: impl Into<PathBuf>) -> Self {
        Self {
            folders: VirtualFolderRepository::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            streams: MediaStreamRepository::new(database),
            probe_path: probe_path.into(),
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
            let mut seen_paths = HashSet::new();
            let mut readable_roots = Vec::new();
            for path in folder.paths {
                let root = Path::new(&path.normalized_path);
                if self
                    .scan_path(root, collection.id, &mut summary, &mut seen_paths)
                    .await?
                {
                    readable_roots.push(root.to_path_buf());
                }
            }
            summary.items_removed += self
                .remove_stale_media(collection.id, &seen_paths, &readable_roots)
                .await?;
        }
        Ok(summary)
    }

    async fn remove_stale_media(
        &self,
        parent_id: Uuid,
        seen_paths: &HashSet<String>,
        readable_roots: &[PathBuf],
    ) -> Result<usize, LibraryScanError> {
        if readable_roots.is_empty() {
            return Ok(0);
        }
        let stale_ids = self
            .items
            .children(parent_id)
            .await?
            .into_iter()
            .filter(|item| is_scanned_media_type(&item.item_type))
            .filter_map(|item| {
                let path = item.path.as_deref()?;
                let scanned_path = item.presentation_unique_key.as_deref() == Some(path);
                let stale = scanned_path
                    && !seen_paths.contains(path)
                    && readable_roots
                        .iter()
                        .any(|root| Path::new(path).starts_with(root));
                stale.then_some(item.id)
            })
            .collect::<Vec<_>>();
        if stale_ids.is_empty() {
            return Ok(0);
        }
        let removed = stale_ids.len();
        self.items.delete_many(&stale_ids).await?;
        Ok(removed)
    }

    async fn scan_path(
        &self,
        root: &Path,
        parent_id: Uuid,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
    ) -> Result<bool, LibraryScanError> {
        let mut entries = match fs::read_dir(root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let mut pending = Vec::new();
        self.scan_entries(&mut entries, &mut pending, parent_id, summary, seen_paths)
            .await?;
        while let Some(directory) = pending.pop() {
            let mut entries = match fs::read_dir(&directory).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            self.scan_entries(&mut entries, &mut pending, parent_id, summary, seen_paths)
                .await?;
        }
        Ok(true)
    }

    async fn scan_entries(
        &self,
        entries: &mut tokio::fs::ReadDir,
        pending: &mut Vec<PathBuf>,
        parent_id: Uuid,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
    ) -> Result<(), LibraryScanError> {
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
            if let Some(path) = path.to_str() {
                seen_paths.insert(path.to_owned());
            }
            if self.ensure_media_item(&path, parent_id, media_kind).await? {
                summary.items_added += 1;
            }
        }
        Ok(())
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

    async fn ensure_media_item(
        &self,
        path: &Path,
        parent_id: Uuid,
        media_kind: MediaKind,
    ) -> Result<bool, LibraryScanError> {
        let Some(path) = path.to_str() else {
            return Ok(false);
        };
        if let Some(mut existing) = self.items.by_paths(&[path.to_owned()]).await?.pop() {
            if let Some(media_info) = self
                .ensure_media_streams(existing.id, path, media_kind)
                .await?
                && apply_probed_item_metadata(&mut existing, path, &media_info)
            {
                self.items.update(existing).await?;
            }
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
        item.data = Some(media_item_data(path, None));
        let mut item = self.items.create(item).await?;
        if let Some(media_info) = self.ensure_media_streams(item.id, path, media_kind).await?
            && apply_probed_item_metadata(&mut item, path, &media_info)
        {
            self.items.update(item).await?;
        }
        Ok(true)
    }

    async fn ensure_media_streams(
        &self,
        item_id: Uuid,
        path: &str,
        media_kind: MediaKind,
    ) -> Result<Option<MediaInfo>, LibraryScanError> {
        let existing = self
            .streams
            .query(MediaStreamQuery {
                item_id,
                stream_index: None,
                stream_type: None,
            })
            .await?;
        let default_stream = default_stream(path, media_kind);
        if !existing.is_empty() && existing != [default_stream.clone()] {
            return Ok(None);
        }
        if let Some(media_info) = self.probe_media_info(path, media_kind).await {
            let probed = streams_from_media_info(&media_info);
            if !probed.is_empty() {
                self.streams.replace(item_id, &probed).await?;
                return Ok(Some(media_info));
            }
            self.streams
                .replace(item_id, std::slice::from_ref(&default_stream))
                .await?;
            return Ok(Some(media_info));
        }
        self.streams
            .replace(item_id, std::slice::from_ref(&default_stream))
            .await?;
        Ok(None)
    }

    async fn probe_media_info(&self, path: &str, media_kind: MediaKind) -> Option<MediaInfo> {
        let probe_path = self.probe_path.clone();
        let path = path.to_owned();
        let log_path = path.clone();
        match tokio::task::spawn_blocking(move || probe_media_info(&probe_path, &path, media_kind))
            .await
        {
            Ok(Ok(media_info)) => Some(media_info),
            Ok(Err(error)) => {
                tracing::debug!(path = log_path, error = %error, "media probe failed during library scan");
                None
            }
            Err(error) => {
                tracing::debug!(path = log_path, error = %error, "media probe task failed during library scan");
                None
            }
        }
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

fn is_scanned_media_type(item_type: &str) -> bool {
    matches!(item_type, "Audio" | "Video")
}

fn apply_probed_item_metadata(
    item: &mut jellyfin_data::entities::base_item::Model,
    path: &str,
    media_info: &MediaInfo,
) -> bool {
    let mut changed = false;
    let data = merged_media_item_data(item.data.as_ref(), path, Some(media_info));
    if item.data.as_ref() != Some(&data) {
        item.data = Some(data);
        changed = true;
    }
    if let Some(runtime_ticks) = media_info.runtime_ticks
        && item.runtime_ticks != Some(runtime_ticks)
    {
        item.runtime_ticks = Some(runtime_ticks);
        changed = true;
    }
    if let Some(production_year) = media_info.production_year
        && item.production_year != Some(production_year)
    {
        item.production_year = Some(production_year);
        changed = true;
    }
    if let Some(premiere_date) = media_info.premiere_date
        && item.premiere_date != Some(premiere_date)
    {
        item.premiere_date = Some(premiere_date);
        changed = true;
    }
    if let Some(overview) = media_info.overview.as_ref()
        && item.overview.as_deref() != Some(overview)
    {
        item.overview = Some(overview.clone());
        changed = true;
    }
    if let Some(sort_name) = media_info.forced_sort_name.as_ref()
        && item.sort_name.as_deref() != Some(sort_name)
    {
        item.sort_name = Some(sort_name.clone());
        changed = true;
    }
    changed
}

fn media_item_data(path: &str, media_info: Option<&MediaInfo>) -> Value {
    merged_media_item_data(None, path, media_info)
}

fn merged_media_item_data(
    existing: Option<&Value>,
    path: &str,
    media_info: Option<&MediaInfo>,
) -> Value {
    let mut object = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    object.insert(
        "Container".to_owned(),
        media_info
            .and_then(|info| info.container.clone())
            .or_else(|| path_extension(path))
            .map_or(Value::Null, Value::String),
    );
    if let Some(bitrate) = media_info.and_then(|info| info.bitrate) {
        object.insert("Bitrate".to_owned(), json!(bitrate));
    } else {
        object.remove("Bitrate");
    }
    Value::Object(object)
}

fn path_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
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

fn probe_media_info(
    probe_path: &Path,
    path: &str,
    media_kind: MediaKind,
) -> Result<MediaInfo, jellyfin_media_encoding::probing::ExternalProbeError> {
    let prober = ExternalSourceProber::new(probe_path, CommandProbeProcessRunner);
    prober.probe(
        &ExternalMediaSource {
            path: path.to_owned(),
            protocol: MediaProtocol::File,
            ..ExternalMediaSource::default()
        },
        &ExternalProbeOptions {
            is_audio: media_kind == MediaKind::Audio,
            ..ExternalProbeOptions::default()
        },
    )
}

fn streams_from_media_info(media_info: &MediaInfo) -> Vec<PersistedMediaStream> {
    media_info
        .media_streams
        .iter()
        .map(stream_from_probe)
        .collect()
}

fn stream_from_probe(stream: &MediaStream) -> PersistedMediaStream {
    PersistedMediaStream {
        stream_index: stream.index,
        stream_type: stream_type_from_probe(stream.stream_type),
        codec: non_empty_string(&stream.codec),
        language: stream.language.clone(),
        channel_layout: None,
        profile: stream.profile.clone(),
        aspect_ratio: stream.aspect_ratio.clone(),
        path: None,
        is_interlaced: Some(stream.is_interlaced()),
        bit_rate: stream.bit_rate.and_then(i32_from_i64),
        channels: stream.channels.and_then(i32_from_u32),
        sample_rate: None,
        is_default: stream.is_default(),
        is_forced: stream.is_forced(),
        is_external: stream.is_external(),
        is_original: stream.is_original(),
        height: stream.height,
        width: stream.width,
        average_frame_rate: stream.average_frame_rate,
        real_frame_rate: stream.real_frame_rate,
        level: stream.level.map(f64_to_f32),
        pixel_format: stream.pixel_format.clone(),
        bit_depth: stream.bit_depth,
        is_anamorphic: Some(stream.is_anamorphic()),
        ref_frames: stream.ref_frames,
        codec_tag: None,
        comment: None,
        nal_length_size: stream.nal_length_size.clone(),
        is_avc: Some(stream.is_avc()),
        title: stream.title.clone(),
        time_base: stream.time_base.clone(),
        codec_time_base: stream.codec_time_base.clone(),
        color_primaries: None,
        color_space: None,
        color_transfer: None,
        dv_version_major: stream.dv_version_major,
        dv_version_minor: stream.dv_version_minor,
        dv_profile: stream.dv_profile,
        dv_level: stream.dv_level,
        rpu_present_flag: stream.rpu_present_flag,
        el_present_flag: stream.el_present_flag,
        bl_present_flag: stream.bl_present_flag,
        dv_bl_signal_compatibility_id: stream.dv_bl_signal_compatibility_id,
        is_hearing_impaired: Some(stream.is_hearing_impaired()),
        rotation: stream.rotation,
        hdr10_plus_present_flag: None,
    }
}

const fn stream_type_from_probe(stream_type: MediaStreamType) -> PersistedMediaStreamType {
    match stream_type {
        MediaStreamType::Audio => PersistedMediaStreamType::Audio,
        MediaStreamType::Video => PersistedMediaStreamType::Video,
        MediaStreamType::Subtitle => PersistedMediaStreamType::Subtitle,
        MediaStreamType::EmbeddedImage => PersistedMediaStreamType::EmbeddedImage,
        MediaStreamType::Data => PersistedMediaStreamType::Data,
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn i32_from_i64(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}

fn i32_from_u32(value: u32) -> Option<i32> {
    i32::try_from(value).ok()
}

#[allow(clippy::cast_possible_truncation)]
fn f64_to_f32(value: f64) -> f32 {
    value as f32
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
        MediaKind, apply_probed_item_metadata, codec_from_extension, default_stream, display_name,
        media_item_data, media_kind, stable_item_id, streams_from_media_info,
    };
    use chrono::Utc;
    use jellyfin_data::{PersistedMediaStreamType, entities::base_item};
    use jellyfin_media_encoding::probing::{ProbeContext, normalize_probe_json};
    use serde_json::json;
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

    #[test]
    fn probed_media_streams_map_to_persisted_streams() {
        let media_info = normalize_probe_json(
            r#"{
                "streams": [
                    {
                        "index": 0,
                        "codec_name": "h264",
                        "codec_type": "video",
                        "profile": "High",
                        "width": 1920,
                        "height": 1080,
                        "display_aspect_ratio": "16:9",
                        "avg_frame_rate": "24000/1001",
                        "r_frame_rate": "24/1",
                        "pix_fmt": "yuv420p",
                        "bits_per_raw_sample": "8",
                        "bit_rate": "5000000",
                        "refs": 4,
                        "level": 41,
                        "disposition": {"default": 1, "forced": 0},
                        "side_data_list": [{"side_data_type": "Display Matrix", "rotation": 90}]
                    },
                    {
                        "index": 1,
                        "codec_name": "aac",
                        "codec_type": "audio",
                        "channels": 2,
                        "tags": {"language": "eng", "title": "Main"}
                    },
                    {
                        "index": 2,
                        "codec_name": "subrip",
                        "codec_type": "subtitle",
                        "disposition": {"hearing_impaired": 1},
                        "tags": {"language": "spa"}
                    }
                ],
                "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "bit_rate": "5500000"}
            }"#,
            ProbeContext {
                path: "/media/Movie.mp4",
                is_audio: false,
            },
        )
        .unwrap();

        let streams = streams_from_media_info(&media_info);

        assert_eq!(streams.len(), 3);
        let video = &streams[0];
        assert_eq!(video.stream_index, 0);
        assert_eq!(video.stream_type, PersistedMediaStreamType::Video);
        assert_eq!(video.codec.as_deref(), Some("h264"));
        assert_eq!(video.profile.as_deref(), Some("High"));
        assert_eq!(video.width, Some(1920));
        assert_eq!(video.height, Some(1080));
        assert_eq!(video.aspect_ratio.as_deref(), Some("16:9"));
        assert_eq!(video.bit_rate, Some(5_000_000));
        assert_eq!(video.ref_frames, Some(4));
        assert_eq!(video.level, Some(41.0));
        assert!(video.is_default);
        assert_eq!(video.rotation, Some(90));

        let audio = &streams[1];
        assert_eq!(audio.stream_type, PersistedMediaStreamType::Audio);
        assert_eq!(audio.codec.as_deref(), Some("aac"));
        assert_eq!(audio.language.as_deref(), Some("eng"));
        assert_eq!(audio.channels, Some(2));
        assert_eq!(audio.title.as_deref(), Some("Main"));

        let subtitle = &streams[2];
        assert_eq!(subtitle.stream_type, PersistedMediaStreamType::Subtitle);
        assert_eq!(subtitle.codec.as_deref(), Some("subrip"));
        assert_eq!(subtitle.language.as_deref(), Some("spa"));
        assert_eq!(subtitle.is_hearing_impaired, Some(true));
    }

    #[test]
    fn probed_streams_drop_persistence_values_that_do_not_fit() {
        let media_info = normalize_probe_json(
            r#"{
                "streams": [{
                    "index": 0,
                    "codec_name": "h264",
                    "codec_type": "video",
                    "bit_rate": "3000000000"
                }],
                "format": {"format_name": "matroska,webm"}
            }"#,
            ProbeContext {
                path: "/media/Huge.mkv",
                is_audio: false,
            },
        )
        .unwrap();

        let streams = streams_from_media_info(&media_info);

        assert_eq!(streams[0].bit_rate, None);
    }

    #[test]
    fn media_item_data_prefers_probe_container_and_bitrate() {
        let media_info = normalize_probe_json(
            r#"{
                "streams": [],
                "format": {
                    "format_name": "matroska,webm",
                    "bit_rate": "5128000",
                    "duration": "300.000000"
                }
            }"#,
            ProbeContext {
                path: "/media/Movie.webm",
                is_audio: false,
            },
        )
        .unwrap();

        assert_eq!(
            media_item_data("/media/Movie.mp4", Some(&media_info)),
            json!({ "Container": "mkv,webm", "Bitrate": 5_128_000_i64 })
        );
        assert_eq!(
            media_item_data("/media/Movie.mp4", None),
            json!({ "Container": "mp4" })
        );
    }

    #[test]
    fn probed_item_metadata_updates_runtime_and_embedded_fields() {
        let media_info = normalize_probe_json(
            r#"{
                "streams": [],
                "format": {
                    "format_name": "matroska,webm",
                    "duration": "300.000000",
                    "tags": {
                        "title-sort": "Fixture Sort",
                        "description": "Fixture overview",
                        "date": "2020-01-02"
                    }
                }
            }"#,
            ProbeContext {
                path: "/media/Movie.mkv",
                is_audio: false,
            },
        )
        .unwrap();
        let now = Utc::now();
        let mut item = base_item::Model {
            id: stable_item_id("/media/Movie.mkv"),
            item_type: "Video".to_owned(),
            data: Some(json!({
                "Container": "mkv",
                "Bitrate": 1,
                "OriginalLanguage": "eng"
            })),
            path: Some("/media/Movie.mkv".to_owned()),
            parent_id: None,
            top_parent_id: None,
            name: Some("Movie".to_owned()),
            clean_name: Some("movie".to_owned()),
            sort_name: Some("Movie".to_owned()),
            media_type: Some("Video".to_owned()),
            overview: None,
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: Some("/media/Movie.mkv".to_owned()),
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: now,
            date_modified: now,
            row_version: 1,
        };

        assert!(apply_probed_item_metadata(
            &mut item,
            "/media/Movie.mkv",
            &media_info
        ));

        assert_eq!(item.runtime_ticks, Some(3_000_000_000));
        assert_eq!(item.production_year, Some(2020));
        assert_eq!(item.overview.as_deref(), Some("Fixture overview"));
        assert_eq!(item.sort_name.as_deref(), Some("Fixture Sort"));
        assert_eq!(
            item.data,
            Some(json!({
                "Container": "mkv,webm",
                "OriginalLanguage": "eng"
            }))
        );
    }
}
