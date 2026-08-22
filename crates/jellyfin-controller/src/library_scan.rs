use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::available_parallelism,
};

use jellyfin_data::{
    BaseItemError, BaseItemImageRepository, BaseItemImageStoreError, BaseItemImageType,
    BaseItemRepository, ChapterRepository, ChapterStoreError, ItemValueRepository,
    KeyframeDataRepository, KeyframeDataStoreError, MediaAttachmentRepository,
    MediaAttachmentStoreError, MediaStreamQuery, MediaStreamRepository, MediaStreamStoreError,
    NewBaseItem, NewBaseItemImage, NewChapter, NewKeyframeData, PersistedMediaAttachment,
    PersistedMediaStream, PersistedMediaStreamType, USER_ROOT_FOLDER_ID, VirtualFolderError,
    VirtualFolderRepository, VirtualFolderWithPaths, entities::base_item,
};
use jellyfin_media_encoding::probing::{
    CommandProbeProcessRunner, ExternalMediaSource, ExternalProbeOptions, ExternalSourceProber,
    MediaAttachment as ProbedMediaAttachment, MediaInfo, MediaProtocol,
    MediaStream as ProbedMediaStream, MediaStreamType,
};
use jellyfin_media_encoding_keyframes::{KeyframeData, extract_keyframes};
use jellyfin_model::{
    CollectionType, MediaProtocol as ModelMediaProtocol, MediaStream as ModelMediaStream,
};
use jellyfin_naming::{ExtraResolver, NamingOptions};
use jellyfin_providers::media_info::{
    MediaFileSystemEntry, SubtitleResolveRequest, SubtitleResolver,
};
use jellyfin_server_implementations::{
    CoreResolutionIgnoreRule, ExtraDirectoryReader, ExtraFileSystemEntry, ExtraMediaKind,
    ExtraOwner, ExtraOwnerKind, LibraryExtrasResolver, ResolutionFileSystemEntry,
    ResolutionParentContext, ResolutionParentKind, ResolvedLibraryExtra,
};
use jellyfin_xbmc_metadata::{
    MovieNfoLocation, MovieVideoType, NfoDocumentKind, NfoMetadata, movie_nfo_save_paths,
    parse_movie_nfo_file,
};
use md5::{Digest, Md5};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{fs, sync::Semaphore};
use uuid::Uuid;

use crate::{LocalizationService, media_streams::MediaStreamMapper};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryScanSummary {
    pub folders_seen: usize,
    pub items_added: usize,
    pub items_removed: usize,
    pub items_seen: usize,
    pub added_ids: Vec<Uuid>,
    pub changed_ids: Vec<Uuid>,
    pub removed_ids: Vec<Uuid>,
}

#[derive(Debug, Error)]
pub enum LibraryScanError {
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    MediaStream(#[from] MediaStreamStoreError),
    #[error(transparent)]
    MediaAttachment(#[from] MediaAttachmentStoreError),
    #[error(transparent)]
    Chapter(#[from] ChapterStoreError),
    #[error(transparent)]
    Keyframe(#[from] KeyframeDataStoreError),
    #[error(transparent)]
    ItemImage(#[from] BaseItemImageStoreError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("a library scan is already in progress")]
    AlreadyScanning,
}

#[derive(Clone)]
pub struct LibraryScanService {
    folders: VirtualFolderRepository,
    items: BaseItemRepository,
    streams: MediaStreamRepository,
    attachments: MediaAttachmentRepository,
    images: BaseItemImageRepository,
    chapters: ChapterRepository,
    keyframes: KeyframeDataRepository,
    values: ItemValueRepository,
    probe_path: PathBuf,
    ffmpeg_path: PathBuf,
    image_cache_directory: PathBuf,
    fanout_concurrency: usize,
    on_progress: Option<Arc<dyn Fn(f64) + Send + Sync>>,
    is_scanning: Arc<AtomicBool>,
}

impl LibraryScanService {
    pub fn set_on_progress(&mut self, callback: Option<Arc<dyn Fn(f64) + Send + Sync>>) {
        self.on_progress = callback;
    }
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self::with_probe_path(database, "ffprobe")
    }

    #[must_use]
    pub fn with_probe_path(database: DatabaseConnection, probe_path: impl Into<PathBuf>) -> Self {
        Self {
            folders: VirtualFolderRepository::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            streams: MediaStreamRepository::new(database.clone()),
            attachments: MediaAttachmentRepository::new(database.clone()),
            images: BaseItemImageRepository::new(database.clone()),
            chapters: ChapterRepository::new(database.clone()),
            keyframes: KeyframeDataRepository::new(database.clone()),
            values: ItemValueRepository::new(database),
            probe_path: probe_path.into(),
            ffmpeg_path: PathBuf::from("ffmpeg"),
            image_cache_directory: PathBuf::from("cache").join("images"),
            fanout_concurrency: default_fanout_concurrency(),
            on_progress: None,
            is_scanning: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_fanout_concurrency(&mut self, concurrency: usize) {
        self.fanout_concurrency = concurrency;
    }

    pub fn set_ffmpeg_path(&mut self, ffmpeg_path: impl Into<PathBuf>) {
        self.ffmpeg_path = ffmpeg_path.into();
    }

    pub fn set_image_cache_directory(&mut self, path: impl Into<PathBuf>) {
        self.image_cache_directory = path.into();
    }

    fn fanout_concurrency(&self) -> usize {
        self.fanout_concurrency.max(1)
    }

    /// Scans configured virtual-folder paths into directly playable base items.
    ///
    /// # Errors
    ///
    /// Returns persistence or file-system errors that prevent the scan from
    /// reading configured paths or writing discovered media.
    fn try_start_scan(&self) -> Result<(), LibraryScanError> {
        self.is_scanning
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| LibraryScanError::AlreadyScanning)
    }

    fn end_scan(&self) {
        self.is_scanning.store(false, Ordering::Release);
    }

    pub async fn scan_all(&self) -> Result<LibraryScanSummary, LibraryScanError> {
        self.try_start_scan()?;
        let result = self.scan_all_inner().await;
        self.end_scan();
        result
    }

    async fn scan_all_inner(&self) -> Result<LibraryScanSummary, LibraryScanError> {
        self.items.ensure_user_root().await?;
        let folders = self.folders.list().await?;
        let total = folders.len();
        let mut summary = LibraryScanSummary::default();
        for (i, folder) in folders.iter().enumerate() {
            self.scan_one_folder(folder, &mut summary).await?;
            if let Some(ref on_progress) = self.on_progress {
                on_progress((i + 1) as f64 / total as f64 * 90.0);
            }
        }
        if let Some(ref on_progress) = self.on_progress {
            on_progress(95.0);
        }
        if let Err(error) = self.values.clear_inherited_tags().await {
            tracing::debug!(%error, "post-scan inherited-tags cleanup failed");
        }
        if let Some(ref on_progress) = self.on_progress {
            on_progress(100.0);
        }
        Ok(summary)
    }

    pub async fn scan_collection(
        &self,
        collection_id: Uuid,
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        self.try_start_scan()?;
        let result = self.scan_collection_inner(collection_id).await;
        self.end_scan();
        result
    }

    async fn scan_collection_inner(
        &self,
        collection_id: Uuid,
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        self.items.ensure_user_root().await?;
        let mut summary = LibraryScanSummary::default();
        let folders = self.folders.list().await?;
        if let Some(folder) = folders.into_iter().find(|f| f.folder.id == collection_id) {
            self.scan_one_folder(&folder, &mut summary).await?;
        }
        if let Some(ref on_progress) = self.on_progress {
            on_progress(95.0);
        }
        if let Err(error) = self.values.clear_inherited_tags().await {
            tracing::debug!(%error, "post-scan inherited-tags cleanup failed");
        }
        if let Some(ref on_progress) = self.on_progress {
            on_progress(100.0);
        }
        Ok(summary)
    }

    async fn scan_one_folder(
        &self,
        folder: &VirtualFolderWithPaths,
        summary: &mut LibraryScanSummary,
    ) -> Result<(), LibraryScanError> {
        let kind = ScanLibraryKind::from_collection_type(folder.folder.collection_type.as_deref());
        let collection = self.ensure_collection_folder(folder).await?;
        summary.folders_seen += 1;
        let mut seen_paths = HashSet::new();
        let mut readable_roots = Vec::new();
        for path in &folder.paths {
            let root = Path::new(&path.normalized_path);
            if self
                .scan_path(root, collection.id, kind, summary, &mut seen_paths)
                .await?
            {
                readable_roots.push(root.to_path_buf());
            }
        }
        let removed = self
            .remove_stale_media(collection.id, &seen_paths, &readable_roots)
            .await?;
        summary.items_removed += removed.len();
        summary.removed_ids.extend(removed);
        Ok(())
    }

    async fn remove_stale_media(
        &self,
        parent_id: Uuid,
        seen_paths: &HashSet<String>,
        readable_roots: &[PathBuf],
    ) -> Result<Vec<Uuid>, LibraryScanError> {
        if readable_roots.is_empty() {
            return Ok(Vec::new());
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
            return Ok(Vec::new());
        }
        self.items.delete_many(&stale_ids).await?;
        Ok(stale_ids)
    }

    async fn scan_path(
        &self,
        root: &Path,
        parent_id: Uuid,
        kind: ScanLibraryKind,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
    ) -> Result<bool, LibraryScanError> {
        let mut entries = match fs::read_dir(root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let mut pending = Vec::new();
        self.scan_entries(
            &mut entries,
            &mut pending,
            parent_id,
            kind,
            summary,
            seen_paths,
        )
        .await?;
        while let Some(directory) = pending.pop() {
            let mut entries = match fs::read_dir(&directory).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            self.scan_entries(
                &mut entries,
                &mut pending,
                parent_id,
                kind,
                summary,
                seen_paths,
            )
            .await?;
        }
        Ok(true)
    }

    async fn scan_entries(
        &self,
        entries: &mut tokio::fs::ReadDir,
        pending: &mut Vec<PathBuf>,
        parent_id: Uuid,
        kind: ScanLibraryKind,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
    ) -> Result<(), LibraryScanError> {
        let mut files = Vec::new();
        let ignore_rule = CoreResolutionIgnoreRule::new(NamingOptions::default(), "");
        let parent_context = ResolutionParentContext::item(ResolutionParentKind::Folder);
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            let path = entry.path();
            if metadata.is_dir() {
                let candidate =
                    ResolutionFileSystemEntry::new(path.to_string_lossy().into_owned(), true);
                if !ignore_rule.should_ignore(&candidate, parent_context)
                    && !is_extras_directory(&path)
                {
                    pending.push(path);
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let candidate =
                ResolutionFileSystemEntry::new(path.to_string_lossy().into_owned(), false);
            if ignore_rule.should_ignore(&candidate, parent_context) {
                continue;
            }
            let Some(media_kind) = media_kind(&path) else {
                continue;
            };
            summary.items_seen += 1;
            if let Some(path) = path.to_str() {
                seen_paths.insert(path.to_owned());
            }
            files.push((path, media_kind));
        }
        if files.is_empty() {
            return Ok(());
        }

        let paths: Vec<String> = files
            .iter()
            .filter_map(|(p, _)| p.to_str().map(String::from))
            .collect();
        let existing = self.items.by_paths(&paths).await?;
        let existing_by_path: HashMap<String, base_item::Model> = existing
            .into_iter()
            .filter_map(|item| Some((item.path.as_deref()?.to_owned(), item)))
            .collect();

        let extra_paths = self.extra_paths_for_entries(&files).await?;
        let regular_files = files
            .iter()
            .filter(|(path, _)| !extra_paths.contains(&path.to_string_lossy().into_owned()))
            .cloned()
            .collect::<Vec<_>>();
        summary.items_added += self
            .process_files(&regular_files, parent_id, kind, &existing_by_path, summary)
            .await?;
        self.ensure_extras(&files, kind, summary, seen_paths)
            .await?;
        Ok(())
    }

    async fn extra_paths_for_entries(
        &self,
        files: &[(PathBuf, MediaKind)],
    ) -> Result<HashSet<String>, LibraryScanError> {
        if files.is_empty() {
            return Ok(HashSet::new());
        }
        let options = NamingOptions::default();
        let resolver = LibraryExtrasResolver::new(options.clone());
        let entries = files
            .iter()
            .filter(|(_, media_kind)| matches!(media_kind, MediaKind::Video | MediaKind::Audio))
            .map(|(path, _)| ExtraFileSystemEntry::new(path.to_string_lossy().into_owned(), false))
            .collect::<Vec<_>>();
        let reader = TokioExtraDirectoryReader;
        let mut extra_paths = HashSet::new();
        for entry in entries.iter().filter(|entry| {
            ExtraResolver::resolve(&entry.full_name, &options)
                .extra_type
                .is_none()
        }) {
            let owner = ExtraOwner::new(
                entry.full_name.clone(),
                display_name(&entry.full_name),
                ExtraOwnerKind::Movie,
            );
            let extras = resolver
                .find_extras(&owner, &entries, &reader)
                .map_err(|error| LibraryScanError::Io(error.into()))?;
            for extra in extras {
                extra_paths.insert(extra.path);
            }
        }
        Ok(extra_paths)
    }

    async fn ensure_extras(
        &self,
        files: &[(PathBuf, MediaKind)],
        kind: ScanLibraryKind,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
    ) -> Result<(), LibraryScanError> {
        if kind.is_tv() {
            return Ok(());
        }
        let options = NamingOptions::default();
        let resolver = LibraryExtrasResolver::new(options.clone());
        let entries = files
            .iter()
            .filter(|(_, media_kind)| matches!(media_kind, MediaKind::Video | MediaKind::Audio))
            .map(|(path, _)| ExtraFileSystemEntry::new(path.to_string_lossy().into_owned(), false))
            .collect::<Vec<_>>();
        let reader = TokioExtraDirectoryReader;
        for entry in entries.iter().filter(|entry| {
            ExtraResolver::resolve(&entry.full_name, &options)
                .extra_type
                .is_none()
        }) {
            let owner = ExtraOwner::new(
                entry.full_name.clone(),
                display_name(&entry.full_name),
                ExtraOwnerKind::Movie,
            );
            let extras = resolver
                .find_extras(&owner, &entries, &reader)
                .map_err(|error| LibraryScanError::Io(error.into()))?;
            for extra in extras {
                seen_paths.insert(extra.path.clone());
                let owner_id = self
                    .items
                    .by_paths(std::slice::from_ref(&entry.full_name))
                    .await?
                    .into_iter()
                    .next()
                    .map(|item| item.id);
                if let Some(owner_id) = owner_id {
                    self.ensure_extra_item(&extra, owner_id, summary).await?;
                }
            }
        }
        Ok(())
    }

    async fn ensure_extra_item(
        &self,
        extra: &ResolvedLibraryExtra,
        owner_id: Uuid,
        summary: &mut LibraryScanSummary,
    ) -> Result<(), LibraryScanError> {
        let item_type = match extra.media_kind {
            ExtraMediaKind::Audio => "Audio",
            ExtraMediaKind::Trailer => "Trailer",
            ExtraMediaKind::Video => "Video",
        };
        let mut data = serde_json::Map::new();
        data.insert(
            "ExtraType".to_owned(),
            json!(extra_type_name(extra.extra_type)),
        );
        let stable_id = stable_item_id(&extra.path, item_type);
        if let Some(existing) = self.items.get(stable_id).await? {
            let mut updated = existing.clone();
            updated.item_type = item_type.to_owned();
            updated.parent_id = Some(owner_id);
            updated.data = Some(serde_json::Value::Object(data));
            self.items.update(updated).await?;
            return Ok(());
        }
        let mut item = NewBaseItem::new(stable_id, item_type);
        item.path = Some(extra.path.clone());
        item.parent_id = Some(owner_id);
        item.name = Some(extra.name.clone());
        item.sort_name = Some(extra.name.clone());
        item.media_type = Some(
            match extra.media_kind {
                ExtraMediaKind::Audio => "Audio",
                _ => "Video",
            }
            .to_owned(),
        );
        item.is_folder = false;
        item.is_virtual_item = false;
        item.presentation_unique_key = Some(extra.path.clone());
        item.data = Some(serde_json::Value::Object(data));
        self.items.create(item).await?;
        summary.items_added += 1;
        Ok(())
    }

    async fn process_files(
        &self,
        files: &[(PathBuf, MediaKind)],
        parent_id: Uuid,
        kind: ScanLibraryKind,
        existing_by_path: &HashMap<String, base_item::Model>,
        summary: &mut LibraryScanSummary,
    ) -> Result<usize, LibraryScanError> {
        if files.is_empty() {
            return Ok(0);
        }
        let concurrency = self.fanout_concurrency();
        if concurrency <= 1 {
            let mut added = 0;
            for (path, media_kind) in files {
                let existing = path.to_str().and_then(|p| existing_by_path.get(p));
                let (item_added, item_id) = self
                    .ensure_media_item(path, parent_id, *media_kind, kind, existing)
                    .await?;
                if item_added {
                    added += 1;
                    summary.added_ids.push(item_id);
                }
                if let Some(path_str) = path.to_str() {
                    let item_type = if kind.is_tv() {
                        media_kind.item_type()
                    } else if *media_kind == MediaKind::Video {
                        kind.video_item_type()
                    } else {
                        media_kind.item_type()
                    };
                    let known_id = existing_by_path
                        .get(path_str)
                        .map(|item| item.id)
                        .unwrap_or_else(|| stable_item_id(path_str, item_type));
                    summary.changed_ids.push(known_id);
                }
            }
            return Ok(added);
        }
        let existing_by_path = Arc::new(
            existing_by_path
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<HashMap<_, _>>(),
        );
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut handles = Vec::with_capacity(files.len());
        for file in files {
            let path = file.0.clone();
            let media_kind = file.1;
            let existing = path.to_str().and_then(|p| existing_by_path.get(p)).cloned();
            let permit = Arc::clone(&semaphore)
                .acquire_owned()
                .await
                .expect("semaphore closed");
            let service = self.clone();
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                service
                    .ensure_media_item(&path, parent_id, media_kind, kind, existing.as_ref())
                    .await
            }));
        }
        let mut added = 0;
        for handle in handles {
            match handle.await {
                Ok(Ok((true, item_id))) => {
                    added += 1;
                    summary.added_ids.push(item_id);
                    summary.changed_ids.push(item_id);
                }
                Ok(Ok((false, item_id))) => summary.changed_ids.push(item_id),
                Ok(Err(error)) => {
                    tracing::debug!(%error, "concurrent media item processing failed");
                }
                Err(error) => {
                    tracing::debug!(%error, "concurrent media task join failed");
                }
            }
        }
        Ok(added)
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

    async fn ensure_media_item<'a>(
        &self,
        path: &Path,
        parent_id: Uuid,
        media_kind: MediaKind,
        kind: ScanLibraryKind,
        existing: Option<&'a base_item::Model>,
    ) -> Result<(bool, Uuid), LibraryScanError> {
        let Some(path_str) = path.to_str() else {
            return Ok((false, Uuid::nil()));
        };
        if let Some(mut existing) = existing.cloned() {
            if !kind.is_tv() {
                let existing_id = existing.id;
                let desired_type = if media_kind == MediaKind::Video {
                    kind.video_item_type()
                } else {
                    media_kind.item_type()
                };
                let mut changed = false;
                if existing.item_type != desired_type {
                    existing.item_type = desired_type.to_owned();
                    changed = true;
                }
                if media_kind.needs_probe() {
                    if let Some(media_info) = self
                        .ensure_media_streams(existing.id, path_str, media_kind)
                        .await?
                        && apply_probed_item_metadata(&mut existing, path_str, &media_info)
                    {
                        changed = true;
                    }
                }
                if apply_nfo_metadata(&mut existing, path_str) {
                    changed = true;
                }
                self.discover_local_images(existing.id, path_str).await?;
                if changed {
                    self.items.update(existing).await?;
                }
                return Ok((false, existing_id));
            }
            return self
                .ensure_episode_item(Some(existing), path, parent_id, media_kind, path_str)
                .await;
        }

        if kind.is_tv() && media_kind == MediaKind::Video {
            return self
                .ensure_episode_item(None, path, parent_id, media_kind, path_str)
                .await;
        }

        let item_type = if media_kind == MediaKind::Video {
            kind.video_item_type()
        } else {
            media_kind.item_type()
        };
        let name = display_name(path_str);
        let mut item = NewBaseItem::new(stable_item_id(path_str, item_type), item_type);
        item.path = Some(path_str.to_owned());
        item.parent_id = Some(parent_id);
        item.name = Some(name.clone());
        item.sort_name = Some(name);
        item.media_type = Some(media_kind.media_type().to_owned());
        item.is_folder = false;
        item.is_virtual_item = false;
        item.presentation_unique_key = Some(path_str.to_owned());
        item.data = Some(media_item_data(path_str, None));
        let mut item = self.items.create(item).await?;
        if media_kind.needs_probe() {
            if let Some(media_info) = self
                .ensure_media_streams(item.id, path_str, media_kind)
                .await?
                && apply_probed_item_metadata(&mut item, path_str, &media_info)
            {
                item = self.items.update(item).await?;
            }
        }
        if apply_nfo_metadata(&mut item, path_str) {
            item = self.items.update(item).await?;
        }
        self.discover_local_images(item.id, path_str).await?;
        Ok((true, item.id))
    }

    async fn ensure_episode_item(
        &self,
        existing: Option<base_item::Model>,
        path: &Path,
        parent_id: Uuid,
        media_kind: MediaKind,
        path_str: &str,
    ) -> Result<(bool, Uuid), LibraryScanError> {
        let ep_result = crate::episode_parser::parse_episode(path);
        let season_number = ep_result.season_number;
        let episode_number = ep_result.episode_number;
        let series_name = ep_result.series_name;

        let mut series_id = None;
        let mut season_id = None;
        let mut series_puk = None;

        if let Some(ref name) = series_name {
            let series_item_type = "Series";
            let series_item_id = stable_item_id(name, series_item_type);
            if self.items.get(series_item_id).await?.is_none() {
                let mut series = NewBaseItem::new(series_item_id, series_item_type);
                series.name = Some(name.clone());
                series.sort_name = Some(name.clone());
                series.is_folder = true;
                series.is_virtual_item = false;
                series.data = Some(json!({ "CollectionType": "tvshows" }));
                self.items.create(series).await?;
            }
            if let Some(mut series) = self.items.get(series_item_id).await?
                && apply_series_nfo_metadata(&mut series, path)
            {
                self.items.update(series).await?;
            }
            series_id = Some(series_item_id);
            series_puk = Some(series_item_id.simple().to_string());

            if let Some(sn) = season_number {
                let season_key = format!("{}_{}", series_item_id.simple(), sn);
                let season_item_id = stable_item_id(&season_key, "Season");
                if self.items.get(season_item_id).await?.is_none() {
                    let mut season = NewBaseItem::new(season_item_id, "Season");
                    season.name = Some(format!("Season {sn}"));
                    season.sort_name = season.name.clone();
                    season.parent_id = Some(series_item_id);
                    season.index_number = Some(sn);
                    season.is_folder = true;
                    season.is_virtual_item = false;
                    season.series_id = Some(series_item_id);
                    season.series_presentation_unique_key = series_puk.clone();
                    self.items.create(season).await?;
                }
                if let Some(mut season) = self.items.get(season_item_id).await?
                    && apply_season_nfo_metadata(&mut season, path, Some(sn))
                {
                    self.items.update(season).await?;
                }
                season_id = Some(season_item_id);
            }
        }

        let item_type = media_kind.item_type();
        let name = display_name(path_str);
        let stable_id = stable_item_id(path_str, item_type);

        if let Some(mut existing) = existing {
            let existing_id = existing.id;
            existing.index_number = episode_number;
            existing.parent_index_number = season_number;
            if let Some(sid) = series_id {
                existing.series_id = Some(sid);
            }
            if let Some(sid) = season_id {
                existing.season_id = Some(sid);
            }
            existing.series_presentation_unique_key = series_puk.clone();
            let nfo_changed = apply_episode_nfo_metadata(&mut existing, path);
            if let Some(media_info) = self
                .ensure_media_streams(existing.id, path_str, media_kind)
                .await?
                && apply_probed_item_metadata(&mut existing, path_str, &media_info)
            {
                self.items.update(existing).await?;
            } else if nfo_changed {
                self.items.update(existing).await?;
            }
            return Ok((false, existing_id));
        }

        let mut item = NewBaseItem::new(stable_id, item_type);
        item.path = Some(path_str.to_owned());
        item.parent_id = Some(parent_id);
        item.name = Some(name.clone());
        item.sort_name = Some(name);
        item.media_type = Some(media_kind.media_type().to_owned());
        item.is_folder = false;
        item.is_virtual_item = false;
        item.presentation_unique_key = Some(path_str.to_owned());
        item.index_number = episode_number;
        item.parent_index_number = season_number;
        item.series_id = series_id;
        item.season_id = season_id;
        item.series_presentation_unique_key = series_puk;
        item.data = Some(media_item_data(path_str, None));
        let mut item = self.items.create(item).await?;
        if apply_episode_nfo_metadata(&mut item, path) {
            item = self.items.update(item).await?;
        }
        if let Some(media_info) = self
            .ensure_media_streams(item.id, path_str, media_kind)
            .await?
            && apply_probed_item_metadata(&mut item, path_str, &media_info)
        {
            let item_id = item.id;
            self.items.update(item).await?;
            return Ok((true, item_id));
        }
        let item_id = item.id;
        Ok((true, item_id))
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
        let media_info = self.probe_media_info(path, media_kind).await;
        let mut streams = media_info
            .as_ref()
            .map(streams_from_media_info)
            .unwrap_or_default();
        if let Some(media_info) = media_info.as_ref() {
            let probed_attachments = attachments_from_media_info(media_info);
            self.attachments
                .replace(item_id, &probed_attachments)
                .await?;
            self.chapters
                .replace(item_id, chapters_from_media_info(media_info))
                .await?;
            self.discover_embedded_images(item_id, path, media_info)
                .await?;
        }
        if media_kind == MediaKind::Video
            && let Some(keyframes) = self.probe_keyframes(path).await
        {
            self.keyframes
                .save(
                    item_id,
                    NewKeyframeData {
                        total_duration: keyframes.total_duration,
                        keyframe_ticks: keyframes.keyframe_ticks,
                    },
                )
                .await?;
        }
        if streams.is_empty() {
            streams.push(default_stream);
        }
        let external_subtitles = self
            .resolve_external_subtitle_streams(path, next_stream_index(&streams))
            .await?;
        streams.extend(external_subtitles);
        self.streams.replace(item_id, &streams).await?;
        Ok(media_info)
    }

    async fn probe_keyframes(&self, path: &str) -> Option<KeyframeData> {
        let probe_path = self.probe_path.clone();
        let path = path.to_owned();
        let log_path = path.clone();
        match tokio::task::spawn_blocking(move || extract_keyframes(&probe_path, &path)).await {
            Ok(Ok(keyframes)) => Some(keyframes),
            Ok(Err(error)) => {
                tracing::debug!(path = log_path, error = %error, "keyframe extraction failed during library scan");
                None
            }
            Err(error) => {
                tracing::debug!(path = log_path, error = %error, "keyframe extraction task failed during library scan");
                None
            }
        }
    }

    async fn discover_local_images(
        &self,
        item_id: Uuid,
        path: &str,
    ) -> Result<(), LibraryScanError> {
        let Some(parent) = Path::new(path).parent() else {
            return Ok(());
        };
        let Ok(mut entries) = fs::read_dir(parent).await else {
            return Ok(());
        };
        let existing = self.images.list(item_id).await?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(image_type) = local_image_type(&name) else {
                continue;
            };
            let path = entry.path();
            let path_string = path.to_string_lossy().into_owned();
            if existing.iter().any(|image| {
                image.image_type == image_type && image.path.eq_ignore_ascii_case(&path_string)
            }) {
                continue;
            }
            let modified = match tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|metadata| metadata.modified().ok())
            {
                Some(modified) => chrono::DateTime::<chrono::Utc>::from(modified),
                None => chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            };
            self.images
                .set_or_append(
                    item_id,
                    NewBaseItemImage {
                        image_type,
                        image_index: 0,
                        path: path_string,
                        date_modified: modified,
                        width: None,
                        height: None,
                        blurhash: None,
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn discover_embedded_images(
        &self,
        item_id: Uuid,
        path: &str,
        media_info: &MediaInfo,
    ) -> Result<(), LibraryScanError> {
        tokio::fs::create_dir_all(&self.image_cache_directory).await?;
        let mut has_primary = false;
        for image in self.images.list(item_id).await? {
            if image.image_type == BaseItemImageType::Primary {
                has_primary = true;
                break;
            }
        }

        for attachment in &media_info.media_attachments {
            let Some(image_type) = attachment_image_type(attachment) else {
                continue;
            };
            let output = self
                .image_cache_directory
                .join(format!("embedded-{item_id}-{}.jpg", attachment.index));
            if self
                .extract_attachment_image(path, attachment.index, &output)
                .await
            {
                self.persist_generated_image(item_id, image_type, &output)
                    .await?;
                if image_type == BaseItemImageType::Primary {
                    has_primary = true;
                }
            }
        }

        if !has_primary
            && let Some(video_stream) = media_info
                .media_streams
                .iter()
                .find(|stream| stream.stream_type == MediaStreamType::Video)
        {
            let offset_ticks = media_info
                .runtime_ticks
                .filter(|runtime| *runtime > 0)
                .map_or(10 * 10_000_000, |runtime| runtime / 10);
            let output = self
                .image_cache_directory
                .join(format!("screenshot-{item_id}-{}.jpg", video_stream.index));
            if self.extract_video_frame(path, offset_ticks, &output).await {
                self.persist_generated_image(item_id, BaseItemImageType::Primary, &output)
                    .await?;
            }
        }
        Ok(())
    }

    async fn extract_attachment_image(
        &self,
        input: &str,
        stream_index: i32,
        output: &Path,
    ) -> bool {
        let status = tokio::process::Command::new(&self.ffmpeg_path)
            .args([
                "-y",
                "-i",
                input,
                "-map",
                &format!("0:{stream_index}"),
                "-frames:v",
                "1",
            ])
            .arg(output)
            .output()
            .await;
        matches!(status, Ok(output) if output.status.success())
    }

    #[allow(clippy::cast_precision_loss)]
    async fn extract_video_frame(&self, input: &str, offset_ticks: i64, output: &Path) -> bool {
        let seconds = format!("{:.6}", offset_ticks as f64 / 10_000_000.0);
        let status = tokio::process::Command::new(&self.ffmpeg_path)
            .args(["-y", "-ss", &seconds, "-i", input, "-frames:v", "1"])
            .arg(output)
            .output()
            .await;
        matches!(status, Ok(output) if output.status.success())
    }

    async fn persist_generated_image(
        &self,
        item_id: Uuid,
        image_type: BaseItemImageType,
        output: &Path,
    ) -> Result<(), LibraryScanError> {
        let output_path = output.to_string_lossy().into_owned();
        let existing = self.images.list(item_id).await?;
        if existing.iter().any(|image| {
            image.image_type == image_type && image.path.eq_ignore_ascii_case(&output_path)
        }) {
            return Ok(());
        }
        let modified = match tokio::fs::metadata(output)
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
        {
            Some(modified) => chrono::DateTime::<chrono::Utc>::from(modified),
            None => chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        };
        self.images
            .set_or_append(
                item_id,
                NewBaseItemImage {
                    image_type,
                    image_index: 0,
                    path: output_path,
                    date_modified: modified,
                    width: None,
                    height: None,
                    blurhash: None,
                },
            )
            .await?;
        Ok(())
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

    async fn resolve_external_subtitle_streams(
        &self,
        path: &str,
        start_index: i32,
    ) -> Result<Vec<PersistedMediaStream>, LibraryScanError> {
        let Some(parent) = Path::new(path).parent() else {
            return Ok(Vec::new());
        };
        let mut entries = match fs::read_dir(parent).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut directory_entries = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            let entry_path = entry.path();
            let Some(path) = entry_path.to_str() else {
                continue;
            };
            let entry = if metadata.is_dir() {
                MediaFileSystemEntry::directory(path)
            } else {
                MediaFileSystemEntry::file(path)
            };
            directory_entries.push(entry);
        }
        Ok(resolve_external_subtitle_streams_from_entries(
            path,
            &directory_entries,
            start_index,
        ))
    }
}

fn default_fanout_concurrency() -> usize {
    available_parallelism()
        .map(|n| n.get().saturating_sub(3).max(1))
        .unwrap_or(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Audio,
    Video,
    Photo,
    Book,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanLibraryKind {
    Movies,
    TvShows,
    Generic,
}

impl ScanLibraryKind {
    fn from_collection_type(collection_type: Option<&str>) -> Self {
        match collection_type
            .and_then(|value| CollectionType::from_str(value).ok())
            .unwrap_or(CollectionType::Unknown)
        {
            CollectionType::Movies | CollectionType::BoxSets => Self::Movies,
            CollectionType::TvShows => Self::TvShows,
            _ => Self::Generic,
        }
    }

    const fn is_tv(self) -> bool {
        matches!(self, Self::TvShows)
    }

    const fn video_item_type(self) -> &'static str {
        match self {
            Self::Movies => "Movie",
            Self::TvShows | Self::Generic => "Video",
        }
    }
}

impl MediaKind {
    const fn item_type(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Photo => "Photo",
            Self::Book => "Book",
        }
    }

    const fn media_type(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Photo => "Photo",
            Self::Book => "Book",
        }
    }

    const fn needs_probe(self) -> bool {
        matches!(self, Self::Audio | Self::Video)
    }
}

struct TokioExtraDirectoryReader;

impl ExtraDirectoryReader for TokioExtraDirectoryReader {
    fn get_files(&self, path: &str) -> std::io::Result<Vec<ExtraFileSystemEntry>> {
        Ok(std::fs::read_dir(path)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_type = entry.file_type().ok()?;
                Some(ExtraFileSystemEntry::new(
                    entry.path().to_string_lossy().into_owned(),
                    file_type.is_dir(),
                ))
            })
            .collect())
    }
}

fn extra_type_name(extra_type: jellyfin_naming::ExtraType) -> &'static str {
    use jellyfin_naming::ExtraType;
    match extra_type {
        ExtraType::BehindTheScenes => "BehindTheScenes",
        ExtraType::Clip => "Clip",
        ExtraType::DeletedScene => "DeletedScene",
        ExtraType::Featurette => "Featurette",
        ExtraType::Interview => "Interview",
        ExtraType::Sample => "Sample",
        ExtraType::Scene => "Scene",
        ExtraType::Short => "Short",
        ExtraType::ThemeSong => "ThemeSong",
        ExtraType::ThemeVideo => "ThemeVideo",
        ExtraType::Trailer => "Trailer",
        ExtraType::Unknown => "Unknown",
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
    if VIDEO_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
    {
        return Some(MediaKind::Video);
    }
    if PHOTO_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
    {
        return Some(MediaKind::Photo);
    }
    if BOOK_EXTENSIONS
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
    {
        return Some(MediaKind::Book);
    }
    None
}

fn local_image_type(file_name: &str) -> Option<BaseItemImageType> {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())?
        .to_ascii_lowercase();
    let stem = stem.trim_end_matches(|character: char| character.is_ascii_digit());
    Some(match stem {
        "poster" | "folder" | "cover" | "default" | "movie" | "show" => BaseItemImageType::Primary,
        "backdrop" | "fanart" | "background" | "art" => BaseItemImageType::Backdrop,
        "logo" | "clearlogo" => BaseItemImageType::Logo,
        "banner" => BaseItemImageType::Banner,
        "landscape" | "thumb" => BaseItemImageType::Thumb,
        "clearart" => BaseItemImageType::Art,
        "disc" | "discart" => BaseItemImageType::Disc,
        "box" => BaseItemImageType::Box,
        "menu" => BaseItemImageType::Menu,
        "back" => BaseItemImageType::BoxRear,
        _ => return None,
    })
}

fn attachment_image_type(attachment: &ProbedMediaAttachment) -> Option<BaseItemImageType> {
    let haystack = attachment
        .file_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let haystack = format!(
        "{haystack} {}",
        attachment.comment.as_deref().unwrap_or_default()
    );
    if contains_any(&haystack, &["poster", "folder", "cover", "default"]) {
        Some(BaseItemImageType::Primary)
    } else if contains_any(&haystack, &["backdrop", "fanart", "background", "art"]) {
        Some(BaseItemImageType::Backdrop)
    } else if contains_any(&haystack, &["logo"]) {
        Some(BaseItemImageType::Logo)
    } else {
        None
    }
}

fn contains_any(value: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|candidate| value.contains(candidate))
}

fn is_scanned_media_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "Audio" | "Video" | "Movie" | "Trailer" | "Photo" | "Book"
    )
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

fn apply_nfo_metadata(item: &mut jellyfin_data::entities::base_item::Model, path: &str) -> bool {
    match item.item_type.as_str() {
        "Movie" | "Video" | "Trailer" | "MusicVideo" => apply_movie_nfo_metadata(item, path),
        _ => false,
    }
}

fn apply_movie_nfo_metadata(
    item: &mut jellyfin_data::entities::base_item::Model,
    media_path: &str,
) -> bool {
    let Some(nfo_path) = movie_nfo_save_paths(&MovieNfoLocation {
        path: PathBuf::from(media_path),
        is_in_mixed_folder: false,
        video_type: MovieVideoType::File,
    })
    .into_iter()
    .find(|path| path.is_file()) else {
        return false;
    };
    let Ok(nfo) = parse_movie_nfo_file(nfo_path) else {
        return false;
    };

    let mut changed = false;
    if let Some(name) = nfo.name.as_deref().filter(|name| !name.is_empty())
        && item.name.as_deref() != Some(name)
    {
        item.name = Some(name.to_owned());
        item.sort_name = Some(name.to_owned());
        changed = true;
    }
    if let Some(overview) = nfo.overview.as_deref().filter(|value| !value.is_empty())
        && item.overview.as_deref() != Some(overview)
    {
        item.overview = Some(overview.to_owned());
        changed = true;
    }
    if let Some(production_year) = nfo.production_year
        && item.production_year != Some(production_year)
    {
        item.production_year = Some(production_year);
        changed = true;
    }
    if let Some(premiere_date) = nfo.premiere_date
        && item.premiere_date.map(|date| date.date_naive()) != Some(premiere_date)
    {
        item.premiere_date = Some(chrono::DateTime::from_naive_utc_and_offset(
            premiere_date.and_hms_opt(0, 0, 0).unwrap_or_default(),
            chrono::Utc,
        ));
        changed = true;
    }
    if let Some(runtime_ticks) = nfo.runtime_ticks
        && item.runtime_ticks != Some(runtime_ticks)
    {
        item.runtime_ticks = Some(runtime_ticks);
        changed = true;
    }
    if let Some(official_rating) = nfo
        .official_rating
        .as_deref()
        .filter(|value| !value.is_empty())
        && item.official_rating.as_deref() != Some(official_rating)
    {
        item.official_rating = Some(official_rating.to_owned());
        changed = true;
    }

    let mut data = metadata_data(item);
    changed |= upsert_string(&mut data, "OriginalTitle", nfo.original_title.as_deref());
    if let Some(tagline) = nfo.tagline.as_deref().filter(|tagline| !tagline.is_empty()) {
        changed |= upsert_string(&mut data, "Tagline", Some(tagline));
    }
    changed |= upsert_f32(&mut data, "CommunityRating", nfo.community_rating);
    changed |= upsert_f32(&mut data, "CriticRating", nfo.critic_rating);
    changed |= upsert_string(&mut data, "CustomRating", nfo.custom_rating.as_deref());
    changed |= upsert_string(
        &mut data,
        "PreferredMetadataLanguage",
        nfo.preferred_metadata_language.as_deref(),
    );
    changed |= upsert_string(
        &mut data,
        "PreferredMetadataCountryCode",
        nfo.preferred_metadata_country_code.as_deref(),
    );
    changed |= upsert_strings(&mut data, "ProductionLocations", &nfo.production_locations);
    changed |= upsert_string(&mut data, "CollectionName", nfo.collection_name.as_deref());
    changed |= upsert_string(&mut data, "AspectRatio", nfo.aspect_ratio.as_deref());
    changed |= upsert_i32(&mut data, "Width", nfo.width);
    changed |= upsert_i32(&mut data, "Height", nfo.height);
    changed |= upsert_bool(&mut data, "HasSubtitles", nfo.has_subtitles);
    changed |= upsert_strings(&mut data, "Genres", &nfo.genres);
    changed |= upsert_strings(&mut data, "Studios", &nfo.studios);
    changed |= upsert_strings(&mut data, "RemoteTrailers", &nfo.remote_trailers);
    if let Some(date_created) = nfo.date_created {
        changed |= upsert_string(&mut data, "DateCreated", Some(&date_created.to_string()));
    }
    if let Some(end_date) = nfo.end_date {
        changed |= upsert_string(
            &mut data,
            "EndDate",
            Some(&end_date.format("%Y-%m-%d").to_string()),
        );
    }
    if !nfo.provider_ids.is_empty() {
        changed |= upsert_value(
            &mut data,
            "ProviderIds",
            serde_json::to_value(&nfo.provider_ids).unwrap_or_default(),
        );
    }
    item.data = Some(Value::Object(data));
    changed
}

fn apply_episode_nfo_metadata(
    item: &mut jellyfin_data::entities::base_item::Model,
    media_path: &Path,
) -> bool {
    let nfo_path = media_path.with_extension("nfo");
    let Ok(input) = std::fs::read_to_string(&nfo_path) else {
        return false;
    };
    let Ok(nfo) = jellyfin_xbmc_metadata::parse_nfo(&input, NfoDocumentKind::Episode) else {
        return false;
    };
    apply_non_movie_nfo(item, &nfo)
}

fn apply_series_nfo_metadata(
    item: &mut jellyfin_data::entities::base_item::Model,
    episode_path: &Path,
) -> bool {
    let Some(directory) = series_directory(episode_path) else {
        return false;
    };
    let Ok(input) = std::fs::read_to_string(directory.join("tvshow.nfo")) else {
        return false;
    };
    let Ok(nfo) = jellyfin_xbmc_metadata::parse_nfo(&input, NfoDocumentKind::Series) else {
        return false;
    };
    apply_non_movie_nfo(item, &nfo)
}

fn apply_season_nfo_metadata(
    item: &mut jellyfin_data::entities::base_item::Model,
    episode_path: &Path,
    season_number: Option<i32>,
) -> bool {
    let Some(directory) = season_directory(episode_path, season_number) else {
        return false;
    };
    let candidate = season_nfo_path(&directory, season_number);
    let Ok(input) = std::fs::read_to_string(candidate) else {
        return false;
    };
    let Ok(nfo) = jellyfin_xbmc_metadata::parse_nfo(&input, NfoDocumentKind::Season) else {
        return false;
    };
    apply_non_movie_nfo(item, &nfo)
}

fn apply_non_movie_nfo(item: &mut base_item::Model, nfo: &NfoMetadata) -> bool {
    let mut changed = false;
    if let Some(name) = nfo.name.as_deref().filter(|name| !name.is_empty())
        && item.name.as_deref() != Some(name)
    {
        item.name = Some(name.to_owned());
        item.sort_name = Some(name.to_owned());
        changed = true;
    }
    if let Some(sort_name) = nfo.sort_name.as_deref().filter(|name| !name.is_empty())
        && item.sort_name.as_deref() != Some(sort_name)
    {
        item.sort_name = Some(sort_name.to_owned());
        changed = true;
    }
    if let Some(overview) = nfo.overview.as_deref().filter(|value| !value.is_empty())
        && item.overview.as_deref() != Some(overview)
    {
        item.overview = Some(overview.to_owned());
        changed = true;
    }
    if let Some(production_year) = nfo.production_year
        && item.production_year != Some(production_year)
    {
        item.production_year = Some(production_year);
        changed = true;
    }
    if let Some(premiere_date) = nfo.premiere_date
        && item.premiere_date.map(|date| date.date_naive()) != Some(premiere_date)
    {
        item.premiere_date = Some(chrono::DateTime::from_naive_utc_and_offset(
            premiere_date.and_hms_opt(0, 0, 0).unwrap_or_default(),
            chrono::Utc,
        ));
        changed = true;
    }
    if let Some(index_number) = nfo.index_number
        && item.index_number != Some(index_number)
    {
        item.index_number = Some(index_number);
        changed = true;
    }
    if let Some(parent_index_number) = nfo.parent_index_number
        && item.parent_index_number != Some(parent_index_number)
    {
        item.parent_index_number = Some(parent_index_number);
        changed = true;
    }
    if nfo.runtime_ticks > 0 && item.runtime_ticks != Some(nfo.runtime_ticks) {
        item.runtime_ticks = Some(nfo.runtime_ticks);
        changed = true;
    }
    if let Some(official_rating) = nfo
        .official_rating
        .as_deref()
        .filter(|value| !value.is_empty())
        && item.official_rating.as_deref() != Some(official_rating)
    {
        item.official_rating = Some(official_rating.to_owned());
        changed = true;
    }

    let mut data = metadata_data(item);
    changed |= upsert_string(&mut data, "OriginalTitle", nfo.original_title.as_deref());
    if !nfo.tagline.is_empty() {
        changed |= upsert_string(&mut data, "Tagline", Some(&nfo.tagline));
    }
    changed |= upsert_strings(&mut data, "Genres", &nfo.genres);
    changed |= upsert_strings(&mut data, "Tags", &nfo.tags);
    changed |= upsert_strings(&mut data, "Studios", &nfo.studios);
    changed |= upsert_i32(&mut data, "IndexNumberEnd", nfo.index_number_end);
    changed |= upsert_i32(
        &mut data,
        "AirsAfterSeasonNumber",
        nfo.airs_after_season_number,
    );
    changed |= upsert_i32(
        &mut data,
        "AirsBeforeSeasonNumber",
        nfo.airs_before_season_number,
    );
    changed |= upsert_i32(
        &mut data,
        "AirsBeforeEpisodeNumber",
        nfo.airs_before_episode_number,
    );
    changed |= upsert_string(&mut data, "AirTime", nfo.air_time.as_deref());
    changed |= upsert_string(
        &mut data,
        "Status",
        nfo.status.as_ref().map(|status| match status {
            jellyfin_xbmc_metadata::SeriesStatus::Continuing => "Continuing",
            jellyfin_xbmc_metadata::SeriesStatus::Ended => "Ended",
            jellyfin_xbmc_metadata::SeriesStatus::Other(value) => value,
        }),
    );
    changed |= upsert_bool(&mut data, "IsLocked", nfo.is_locked);
    if !nfo.air_days.is_empty() {
        changed |= upsert_strings(
            &mut data,
            "AirDays",
            &nfo.air_days
                .iter()
                .map(|day| weekday_name(*day).to_owned())
                .collect::<Vec<_>>(),
        );
    }
    if let Some(date_created) = nfo.date_created {
        changed |= upsert_string(&mut data, "DateCreated", Some(&date_created.to_string()));
    }
    if !nfo.provider_ids.is_empty() {
        changed |= upsert_value(
            &mut data,
            "ProviderIds",
            serde_json::to_value(&nfo.provider_ids).unwrap_or_default(),
        );
    }
    item.data = Some(Value::Object(data));
    changed
}

fn metadata_data(item: &base_item::Model) -> serde_json::Map<String, Value> {
    item.data
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn upsert_string(
    data: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<&str>,
) -> bool {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return false;
    };
    upsert_value(data, key, json!(value))
}

fn upsert_strings(data: &mut serde_json::Map<String, Value>, key: &str, values: &[String]) -> bool {
    if values.is_empty() {
        return false;
    }
    upsert_value(
        data,
        key,
        Value::Array(values.iter().cloned().map(Value::String).collect()),
    )
}

fn upsert_i32(data: &mut serde_json::Map<String, Value>, key: &str, value: Option<i32>) -> bool {
    value.is_some_and(|value| upsert_value(data, key, json!(value)))
}

fn upsert_f32(data: &mut serde_json::Map<String, Value>, key: &str, value: Option<f32>) -> bool {
    value.is_some_and(|value| upsert_value(data, key, json!(value)))
}

fn upsert_bool(data: &mut serde_json::Map<String, Value>, key: &str, value: bool) -> bool {
    upsert_value(data, key, json!(value))
}

fn upsert_value(data: &mut serde_json::Map<String, Value>, key: &str, value: Value) -> bool {
    if data.get(key) == Some(&value) {
        return false;
    }
    data.insert(key.to_owned(), value);
    true
}

fn series_directory(episode_path: &Path) -> Option<PathBuf> {
    let parent = episode_path.parent()?;
    let season = parent.file_name()?.to_str()?;
    if crate::episode_parser::parse_season_directory(season).is_some() {
        parent.parent().map(Path::to_path_buf)
    } else {
        Some(parent.to_path_buf())
    }
}

fn season_directory(episode_path: &Path, season_number: Option<i32>) -> Option<PathBuf> {
    let parent = episode_path.parent()?;
    let season = parent.file_name()?.to_str()?;
    if crate::episode_parser::parse_season_directory(season).is_some() {
        Some(parent.to_path_buf())
    } else {
        let series = series_directory(episode_path)?;
        let Some(season_number) = season_number else {
            return None;
        };
        let candidate = series.join(format!("Season {season_number}"));
        candidate.is_dir().then_some(candidate)
    }
}

fn season_nfo_path(directory: &Path, season_number: Option<i32>) -> PathBuf {
    let Some(season_number) = season_number else {
        return directory.join("season0.nfo");
    };
    let candidates = [
        format!("Season {season_number:02}.nfo"),
        format!("Season {season_number}.nfo"),
        format!("season{season_number}.nfo"),
    ];
    for name in candidates {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    directory.join(format!("Season {season_number:02}.nfo"))
}

const fn weekday_name(day: chrono::Weekday) -> &'static str {
    match day {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
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
            MediaKind::Video | MediaKind::Photo | MediaKind::Book => {
                PersistedMediaStreamType::Video
            }
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

fn attachments_from_media_info(media_info: &MediaInfo) -> Vec<PersistedMediaAttachment> {
    media_info
        .media_attachments
        .iter()
        .map(attachment_from_probe)
        .collect()
}

fn chapters_from_media_info(media_info: &MediaInfo) -> Vec<NewChapter> {
    let runtime_ticks = media_info.runtime_ticks.unwrap_or_default();
    media_info
        .chapters
        .iter()
        .enumerate()
        .map(|(index, chapter)| {
            let end_position_ticks = media_info
                .chapters
                .get(index + 1)
                .map_or(runtime_ticks, |next| next.start_position_ticks);
            NewChapter {
                index_number: i32::try_from(index).unwrap_or(i32::MAX),
                start_position_ticks: chapter.start_position_ticks,
                end_position_ticks: end_position_ticks.max(chapter.start_position_ticks),
                name: chapter.name.clone(),
            }
        })
        .collect()
}

fn resolve_external_subtitle_streams_from_entries(
    path: &str,
    directory_entries: &[MediaFileSystemEntry],
    start_index: i32,
) -> Vec<PersistedMediaStream> {
    let localization = LocalizationService;
    let resolver = SubtitleResolver::new(NamingOptions::default(), &localization);
    let mapper = MediaStreamMapper::default();
    resolver
        .resolve(SubtitleResolveRequest {
            media_path: path,
            protocol: ModelMediaProtocol::File,
            media_is_directory: false,
            containing_directory_exists: true,
            directory_entries,
            metadata_directory_exists: false,
            metadata_entries: &[],
            start_index,
        })
        .into_iter()
        .map(|resolved| external_stream_from_resolved(resolved.stream, &mapper))
        .collect()
}

fn external_stream_from_resolved(
    mut stream: ModelMediaStream,
    mapper: &MediaStreamMapper,
) -> PersistedMediaStream {
    if stream.codec.as_deref().is_none_or(str::is_empty) {
        stream.codec = stream.path.as_deref().and_then(path_extension);
    }
    mapper.to_persisted(&stream)
}

fn attachment_from_probe(attachment: &ProbedMediaAttachment) -> PersistedMediaAttachment {
    PersistedMediaAttachment {
        attachment_index: attachment.index,
        codec: Some(attachment.codec.clone()),
        codec_tag: attachment.codec_tag.clone(),
        comment: attachment.comment.clone(),
        file_name: attachment.file_name.clone(),
        mime_type: attachment.mime_type.clone(),
        delivery_url: None,
    }
}

fn stream_from_probe(stream: &ProbedMediaStream) -> PersistedMediaStream {
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

fn next_stream_index(streams: &[PersistedMediaStream]) -> i32 {
    streams
        .iter()
        .filter_map(|stream| stream.stream_index.checked_add(1))
        .max()
        .unwrap_or_default()
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

fn stable_item_id(path: &str, item_type: &str) -> Uuid {
    let digest = Md5::digest(format!("{item_type}{path}"));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest);
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn is_extras_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    NamingOptions::default()
        .video_extra_rules
        .iter()
        .any(|rule| {
            rule.rule_type == jellyfin_naming::ExtraRuleType::DirectoryName
                && rule.token.eq_ignore_ascii_case(name)
        })
}

const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aiff", "ape", "dsf", "flac", "m4a", "m4b", "mp3", "ogg", "opus", "wav", "wma",
];

const VIDEO_EXTENSIONS: &[&str] = &[
    "avi", "divx", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "ts", "webm",
    "wmv",
];

const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "svg", "ico",
];

const BOOK_EXTENSIONS: &[&str] = &["pdf", "epub", "mobi", "cbr", "cbz", "cb7", "cbt", "djvu"];

#[cfg(test)]
mod tests {
    use super::{
        MediaKind, ScanLibraryKind, apply_non_movie_nfo, apply_probed_item_metadata,
        attachment_image_type, attachments_from_media_info, codec_from_extension, default_stream,
        display_name, extra_type_name, is_extras_directory, local_image_type, media_item_data,
        media_kind, next_stream_index, resolve_external_subtitle_streams_from_entries,
        stable_item_id, streams_from_media_info,
    };
    use chrono::Utc;
    use jellyfin_data::{PersistedMediaStreamType, entities::base_item};
    use jellyfin_media_encoding::probing::{ProbeContext, normalize_probe_json};
    use jellyfin_naming::ExtraType;
    use jellyfin_providers::media_info::MediaFileSystemEntry;
    use jellyfin_xbmc_metadata::NfoMetadata;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn media_kind_accepts_common_direct_play_extensions() {
        assert_eq!(media_kind(Path::new("movie.MKV")), Some(MediaKind::Video));
        assert_eq!(media_kind(Path::new("song.FlAc")), Some(MediaKind::Audio));
        assert_eq!(media_kind(Path::new("photo.jpg")), Some(MediaKind::Photo));
        assert_eq!(media_kind(Path::new("book.pdf")), Some(MediaKind::Book));
        assert_eq!(media_kind(Path::new("data.nfo")), None);
    }

    #[test]
    fn non_movie_nfo_applies_series_and_episode_fields() {
        let nfo = NfoMetadata {
            name: Some("Show Name".to_owned()),
            original_title: Some("Original Show".to_owned()),
            overview: Some("Overview".to_owned()),
            genres: vec!["Drama".to_owned()],
            tags: vec!["Rewatch".to_owned()],
            studios: vec!["Studio".to_owned()],
            provider_ids: std::collections::HashMap::from([(
                "Tmdb".to_owned(),
                "12345".to_owned(),
            )]),
            status: Some(jellyfin_xbmc_metadata::SeriesStatus::Ended),
            is_locked: true,
            ..NfoMetadata::default()
        };
        let mut item = base_item::Model {
            id: uuid::Uuid::new_v4(),
            item_type: "Series".to_owned(),
            data: None,
            path: None,
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
            is_folder: true,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: Utc::now(),
            date_modified: Utc::now(),
            row_version: 1,
        };

        assert!(apply_non_movie_nfo(&mut item, &nfo));
        assert_eq!(item.name.as_deref(), Some("Show Name"));
        assert_eq!(item.overview.as_deref(), Some("Overview"));
        let data = item.data.as_ref().unwrap();
        assert_eq!(data["OriginalTitle"], "Original Show");
        assert_eq!(data["Genres"][0], "Drama");
        assert_eq!(data["Tags"][0], "Rewatch");
        assert_eq!(data["Studios"][0], "Studio");
        assert_eq!(data["ProviderIds"]["Tmdb"], "12345");
        assert_eq!(data["Status"], "Ended");
        assert_eq!(data["IsLocked"], true);
    }

    #[test]
    fn local_image_names_map_to_official_image_types() {
        use jellyfin_data::BaseItemImageType;

        assert_eq!(
            local_image_type("poster.jpg"),
            Some(BaseItemImageType::Primary)
        );
        assert_eq!(
            local_image_type("folder.png"),
            Some(BaseItemImageType::Primary)
        );
        assert_eq!(
            local_image_type("fanart1.jpg"),
            Some(BaseItemImageType::Backdrop)
        );
        assert_eq!(local_image_type("logo.png"), Some(BaseItemImageType::Logo));
        assert_eq!(
            local_image_type("landscape.jpg"),
            Some(BaseItemImageType::Thumb)
        );
        assert_eq!(
            local_image_type("clearart.png"),
            Some(BaseItemImageType::Art)
        );
        assert_eq!(local_image_type("unrelated.txt"), None);
    }

    #[test]
    fn embedded_attachment_names_select_official_image_types() {
        use jellyfin_data::BaseItemImageType;
        use jellyfin_media_encoding::probing::MediaAttachment;

        let attachment = |name: Option<&str>| MediaAttachment {
            codec: "mjpeg".to_owned(),
            index: 1,
            codec_tag: None,
            file_name: name.map(str::to_owned),
            mime_type: Some("image/jpeg".to_owned()),
            comment: None,
        };
        assert_eq!(
            attachment_image_type(&attachment(Some("poster.jpg"))),
            Some(BaseItemImageType::Primary)
        );
        assert_eq!(
            attachment_image_type(&attachment(Some("fanart.jpg"))),
            Some(BaseItemImageType::Backdrop)
        );
        assert_eq!(
            attachment_image_type(&attachment(Some("logo.png"))),
            Some(BaseItemImageType::Logo)
        );
        assert_eq!(attachment_image_type(&attachment(Some("other.bin"))), None);
    }

    #[test]
    fn video_library_kind_matches_official_jellyfin_type_resolution() {
        assert_eq!(
            ScanLibraryKind::from_collection_type(Some("movies")).video_item_type(),
            "Movie"
        );
        assert_eq!(
            ScanLibraryKind::from_collection_type(Some("boxsets")).video_item_type(),
            "Movie"
        );
        assert_eq!(
            ScanLibraryKind::from_collection_type(Some("homevideos")).video_item_type(),
            "Video"
        );
        assert_eq!(
            ScanLibraryKind::from_collection_type(Some("musicvideos")).video_item_type(),
            "Video"
        );
        assert_eq!(
            ScanLibraryKind::from_collection_type(None).video_item_type(),
            "Video"
        );
        assert!(ScanLibraryKind::from_collection_type(Some("tvshows")).is_tv());
        assert!(!ScanLibraryKind::from_collection_type(Some("movies")).is_tv());
    }

    #[test]
    fn extras_directories_and_types_follow_official_naming_rules() {
        assert!(is_extras_directory(std::path::Path::new(
            "/movies/Up/extras"
        )));
        assert!(is_extras_directory(std::path::Path::new(
            "/movies/Up/TRAILERS"
        )));
        assert!(is_extras_directory(std::path::Path::new(
            "/movies/Up/behind the scenes"
        )));
        assert!(!is_extras_directory(std::path::Path::new(
            "/movies/Up/regular-folder"
        )));
        assert_eq!(extra_type_name(ExtraType::ThemeSong), "ThemeSong");
        assert_eq!(extra_type_name(ExtraType::ThemeVideo), "ThemeVideo");
        assert_eq!(extra_type_name(ExtraType::Trailer), "Trailer");
        assert_eq!(extra_type_name(ExtraType::Featurette), "Featurette");
    }

    #[test]
    fn stable_ids_are_path_specific_and_repeatable() {
        assert_eq!(
            stable_item_id("/media/movie.mkv", "Video"),
            stable_item_id("/media/movie.mkv", "Video")
        );
        assert_ne!(
            stable_item_id("/media/a.mkv", "Video"),
            stable_item_id("/media/b.mkv", "Video")
        );
        assert_ne!(
            stable_item_id("/media/file.mkv", "Video"),
            stable_item_id("/media/file.mkv", "Audio")
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
    fn external_subtitle_sidecars_map_to_persisted_streams_after_probe_indexes() {
        let entries = [
            MediaFileSystemEntry::file("/media/Movie.eng.default.srt"),
            MediaFileSystemEntry::file("/media/Movie.Commentary.forced.sdh.en.ass"),
            MediaFileSystemEntry::file("/media/MovieSequel.en.srt"),
            MediaFileSystemEntry::directory("/media/Movie.fra.srt"),
        ];

        let streams =
            resolve_external_subtitle_streams_from_entries("/media/Movie.mkv", &entries, 2);

        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].stream_index, 2);
        assert_eq!(streams[0].stream_type, PersistedMediaStreamType::Subtitle);
        assert_eq!(streams[0].codec.as_deref(), Some("srt"));
        assert_eq!(streams[0].language.as_deref(), Some("eng"));
        assert!(streams[0].is_default);
        assert!(!streams[0].is_forced);
        assert!(!streams[0].is_hearing_impaired.unwrap_or_default());
        assert_eq!(
            streams[0].path.as_deref(),
            Some("/media/Movie.eng.default.srt")
        );

        assert_eq!(streams[1].stream_index, 3);
        assert_eq!(streams[1].codec.as_deref(), Some("ass"));
        assert_eq!(streams[1].language.as_deref(), Some("eng"));
        assert_eq!(streams[1].title.as_deref(), Some("Commentary"));
        assert!(streams[1].is_forced);
        assert!(streams[1].is_hearing_impaired.unwrap_or_default());
    }

    #[test]
    fn next_stream_index_uses_one_after_highest_persisted_index() {
        let mut streams = vec![
            default_stream("/media/Movie.mkv", MediaKind::Video),
            default_stream("/media/Movie.eng.srt", MediaKind::Video),
        ];
        streams[1].stream_index = 7;

        assert_eq!(next_stream_index(&streams), 8);
        assert_eq!(next_stream_index(&[]), 0);
    }

    #[test]
    fn probed_attachments_map_to_persisted_rows() {
        let media_info = normalize_probe_json(
            r#"{
                "streams": [{
                    "index": 4,
                    "codec_name": "ttf",
                    "codec_type": "attachment",
                    "codec_tag_string": "[0][0][0][0]",
                    "tags": {
                        "filename": "font.ttf",
                        "mimetype": "font/ttf",
                        "comment": "Font"
                    }
                }]
            }"#,
            ProbeContext {
                path: "/media/Movie.mkv",
                is_audio: false,
            },
        )
        .unwrap();

        let attachments = attachments_from_media_info(&media_info);

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].attachment_index, 4);
        assert_eq!(attachments[0].codec.as_deref(), Some("ttf"));
        assert_eq!(attachments[0].codec_tag.as_deref(), Some("[0][0][0][0]"));
        assert_eq!(attachments[0].file_name.as_deref(), Some("font.ttf"));
        assert_eq!(attachments[0].mime_type.as_deref(), Some("font/ttf"));
        assert_eq!(attachments[0].comment.as_deref(), Some("Font"));
        assert_eq!(attachments[0].delivery_url, None);
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
            id: stable_item_id("/media/Movie.mkv", "Video"),
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
