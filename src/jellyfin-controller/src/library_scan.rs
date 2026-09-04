use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex, RwLock},
    thread::available_parallelism,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use jellyfin_data::{
    BaseItemError, BaseItemImageRepository, BaseItemImageStoreError, BaseItemImageType,
    BaseItemRepository, ChapterRepository, ChapterStoreError, ItemMetadataPatch,
    ItemUpdateRepository, ItemUpdateStoreError, ItemValueError, ItemValueRepository,
    MediaAttachmentRepository, MediaAttachmentStoreError, MediaStreamQuery, MediaStreamRepository,
    MediaStreamStoreError, NewBaseItem, NewBaseItemImage, NewChapter, NewPerson,
    PersistedMediaAttachment, PersistedMediaStream, PersistedMediaStreamType,
    PersonError as PersonStoreError, PersonRepository, USER_ROOT_FOLDER_ID, VirtualFolderError,
    VirtualFolderRepository, VirtualFolderWithPaths,
    entities::{base_item, item_value::ItemValueType},
};
use jellyfin_media_encoding::probing::{
    CommandProbeProcessRunner, ExternalMediaSource, ExternalProbeOptions, ExternalSourceProber,
    MediaAttachment as ProbedMediaAttachment, MediaInfo, MediaProtocol,
    MediaStream as ProbedMediaStream, MediaStreamType,
};
use jellyfin_model::{
    CollectionType, MediaProtocol as ModelMediaProtocol, MediaStream as ModelMediaStream,
};
use jellyfin_naming::{ExtraResolver, NamingOptions, VideoListResolver, VideoResolver};
use jellyfin_providers::media_info::{
    MediaFileSystemEntry, SubtitleResolveRequest, SubtitleResolver,
};
use jellyfin_server_implementations::{
    CoreResolutionIgnoreRule, ExtraDirectoryReader, ExtraFileSystemEntry, ExtraMediaKind,
    ExtraOwner, ExtraOwnerKind, FilesystemDirectoryReader, LibraryExtrasResolver,
    LibraryParentKind, LibraryResolveArgs, LibraryResolverChain, ResolutionFileSystemEntry,
    ResolutionParentContext, ResolutionParentKind, ResolvedLibraryExtra, ResolvedLibraryItemKind,
};
use jellyfin_xbmc_metadata::{
    MovieNfo, MovieNfoLocation, MovieVideoType, NfoDocumentKind, NfoMetadata, NfoPerson,
    PersonKind as NfoPersonKind, movie_nfo_save_paths, parse_movie_nfo_file, parse_nfo,
};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{fs, sync::Notify};
use uuid::Uuid;

use crate::{
    LocalizationService, episode_parser::parse_season_directory, media_streams::MediaStreamMapper,
};

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
    ItemImage(#[from] BaseItemImageStoreError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error(transparent)]
    ItemUpdate(#[from] ItemUpdateStoreError),
    #[error(transparent)]
    Person(#[from] PersonStoreError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("a library scan is already in progress")]
    AlreadyScanning,
}

pub struct LibraryScanService {
    folders: VirtualFolderRepository,
    items: BaseItemRepository,
    streams: MediaStreamRepository,
    attachments: MediaAttachmentRepository,
    images: BaseItemImageRepository,
    people: PersonRepository,
    updates: ItemUpdateRepository,
    chapters: ChapterRepository,
    values: ItemValueRepository,
    probe_path: Arc<PathBuf>,
    ffmpeg_path: RwLock<Arc<PathBuf>>,
    image_cache_directory: RwLock<Arc<PathBuf>>,
    media_item_limiter: MediaItemConcurrencyLimiter,
    active_scans: Arc<Mutex<HashSet<Uuid>>>,
}

#[derive(Debug)]
struct MediaItemConcurrencyLimiter {
    state: Mutex<MediaItemConcurrencyState>,
    changed: Notify,
}

#[derive(Debug)]
struct MediaItemConcurrencyState {
    limit: usize,
    in_flight: usize,
}

impl MediaItemConcurrencyLimiter {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(MediaItemConcurrencyState {
                limit: limit.max(1),
                in_flight: 0,
            }),
            changed: Notify::new(),
        }
    }

    fn limit(&self) -> usize {
        self.state
            .lock()
            .expect("library scan media-item concurrency lock poisoned")
            .limit
    }

    fn set_limit(&self, limit: usize) {
        self.state
            .lock()
            .expect("library scan media-item concurrency lock poisoned")
            .limit = limit.max(1);
        self.changed.notify_waiters();
    }

    async fn acquire(&self) -> MediaItemConcurrencyPermit<'_> {
        loop {
            // Register before checking the state so an increase or release
            // cannot be lost between observing a full pool and awaiting it.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();
            {
                let mut state = self
                    .state
                    .lock()
                    .expect("library scan media-item concurrency lock poisoned");
                if state.in_flight < state.limit {
                    state.in_flight += 1;
                    return MediaItemConcurrencyPermit { limiter: self };
                }
            }
            notified.await;
        }
    }
}

struct MediaItemConcurrencyPermit<'a> {
    limiter: &'a MediaItemConcurrencyLimiter,
}

impl Drop for MediaItemConcurrencyPermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .limiter
            .state
            .lock()
            .expect("library scan media-item concurrency lock poisoned");
        state.in_flight = state.in_flight.saturating_sub(1);
        drop(state);
        self.limiter.changed.notify_one();
    }
}

struct LibraryScanGuard {
    active_scans: Arc<Mutex<HashSet<Uuid>>>,
    collection_ids: Vec<Uuid>,
}

impl Drop for LibraryScanGuard {
    fn drop(&mut self) {
        let mut active_scans = self
            .active_scans
            .lock()
            .expect("library active scan lock poisoned");
        for collection_id in &self.collection_ids {
            active_scans.remove(collection_id);
        }
    }
}

impl LibraryScanService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self::with_probe_path(database, "ffprobe")
    }

    #[must_use]
    pub fn with_probe_path(
        database: impl Into<jellyfin_data::SharedDatabase>,
        probe_path: impl Into<PathBuf>,
    ) -> Self {
        let database = database.into();
        Self {
            folders: VirtualFolderRepository::new(Arc::clone(&database)),
            items: BaseItemRepository::new(Arc::clone(&database)),
            streams: MediaStreamRepository::new(Arc::clone(&database)),
            attachments: MediaAttachmentRepository::new(Arc::clone(&database)),
            images: BaseItemImageRepository::new(Arc::clone(&database)),
            people: PersonRepository::new(Arc::clone(&database)),
            updates: ItemUpdateRepository::new(Arc::clone(&database)),
            chapters: ChapterRepository::new(Arc::clone(&database)),
            values: ItemValueRepository::new(database),
            probe_path: Arc::new(probe_path.into()),
            ffmpeg_path: RwLock::new(Arc::new(PathBuf::from("ffmpeg"))),
            image_cache_directory: RwLock::new(Arc::new(PathBuf::from("cache").join("images"))),
            media_item_limiter: MediaItemConcurrencyLimiter::new(default_fanout_concurrency()),
            active_scans: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn set_fanout_concurrency(&self, concurrency: usize) {
        self.media_item_limiter.set_limit(concurrency);
    }

    pub fn set_ffmpeg_path(&self, ffmpeg_path: impl Into<PathBuf>) {
        self.set_shared_ffmpeg_path(Arc::new(ffmpeg_path.into()));
    }

    pub fn set_shared_ffmpeg_path(&self, ffmpeg_path: Arc<PathBuf>) {
        *self
            .ffmpeg_path
            .write()
            .expect("library scan FFmpeg path lock poisoned") = ffmpeg_path;
    }

    pub fn set_image_cache_directory(&self, path: impl Into<PathBuf>) {
        *self
            .image_cache_directory
            .write()
            .expect("library scan image cache path lock poisoned") = Arc::new(path.into());
    }

    fn fanout_concurrency(&self) -> usize {
        self.media_item_limiter.limit()
    }

    fn ffmpeg_path(&self) -> Arc<PathBuf> {
        Arc::clone(
            &self
                .ffmpeg_path
                .read()
                .expect("library scan FFmpeg path lock poisoned"),
        )
    }

    fn image_cache_directory(&self) -> Arc<PathBuf> {
        Arc::clone(
            &self
                .image_cache_directory
                .read()
                .expect("library scan image cache path lock poisoned"),
        )
    }

    /// Scans configured virtual-folder paths into directly playable base items.
    ///
    /// # Errors
    ///
    /// Returns persistence or file-system errors that prevent the scan from
    /// reading configured paths or writing discovered media.
    fn try_start_scans(
        &self,
        collection_ids: impl IntoIterator<Item = Uuid>,
    ) -> Result<LibraryScanGuard, LibraryScanError> {
        let collection_ids = collection_ids.into_iter().collect::<HashSet<_>>();
        let mut active_scans = self
            .active_scans
            .lock()
            .expect("library active scan lock poisoned");
        if collection_ids
            .iter()
            .any(|collection_id| active_scans.contains(collection_id))
        {
            return Err(LibraryScanError::AlreadyScanning);
        }
        active_scans.extend(collection_ids.iter().copied());
        drop(active_scans);
        Ok(LibraryScanGuard {
            active_scans: Arc::clone(&self.active_scans),
            collection_ids: collection_ids.into_iter().collect(),
        })
    }

    /// Scans every configured library collection.
    ///
    /// # Errors
    ///
    /// Returns an error when any library scan fails.
    pub async fn scan_all(&self) -> Result<LibraryScanSummary, LibraryScanError> {
        self.run_scan_all(None).await
    }

    /// Scans every configured library collection and reports progress.
    ///
    /// # Errors
    ///
    /// Returns an error when any library scan fails.
    pub async fn scan_all_with_progress(
        &self,
        on_progress: &(dyn Fn(f64) + Send + Sync),
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        self.run_scan_all(Some(on_progress)).await
    }

    /// Replaces the lightweight stream placeholder created for a `.strm`
    /// item with stream details probed from its resolved media target.
    ///
    /// Library scans deliberately avoid opening every remote `.strm` target.
    /// Playback negotiation, however, needs the real video and audio codecs to
    /// decide whether a client can direct-play the file. This method performs
    /// that work once, immediately before the first playback-info response,
    /// and persists the result for subsequent requests.
    ///
    /// # Errors
    ///
    /// Returns persistence or sidecar-discovery errors. Probe failures are
    /// best-effort and leave the existing placeholder intact.
    pub async fn hydrate_strm_media_streams(
        &self,
        item_id: Uuid,
    ) -> Result<bool, LibraryScanError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        let Some(sidecar_path) = item
            .path
            .as_deref()
            .filter(|path| is_strm_path(Path::new(path)))
        else {
            return Ok(false);
        };
        let Some(target) = item
            .data
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|data| data.get("StrmTarget").or_else(|| data.get("strm_target")))
            .and_then(Value::as_str)
            .filter(|target| !target.is_empty())
        else {
            return Ok(false);
        };
        let Some(kind) = media_kind(Path::new(target)).filter(|kind| kind.needs_probe()) else {
            return Ok(false);
        };

        let existing = self
            .streams
            .query(MediaStreamQuery {
                item_id,
                stream_index: None,
                stream_type: None,
            })
            .await?;
        let placeholder = default_stream(target, kind);
        let embedded_streams = existing
            .iter()
            .filter(|stream| !stream.is_external)
            .collect::<Vec<_>>();
        if embedded_streams.len() != 1 || embedded_streams[0] != &placeholder {
            return Ok(false);
        }

        let Some(mut media_info) = self.probe_media_info(target, kind).await else {
            return Ok(false);
        };
        let mut streams = streams_from_media_info(&mut media_info);
        if streams.is_empty() {
            return Ok(false);
        }
        streams.extend(
            self.resolve_external_subtitle_streams(sidecar_path, next_stream_index(&streams))
                .await?,
        );
        self.streams.replace(item_id, &streams).await?;
        Ok(true)
    }

    async fn run_scan_all(
        &self,
        on_progress: Option<&(dyn Fn(f64) + Send + Sync)>,
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        self.items.ensure_user_root().await?;
        let folders = self.folders.list().await?;
        // Reserve all collections atomically so an overlapping single-library
        // scan cannot begin midway through a full scan. Cancellation drops the
        // guard and releases every reservation.
        let _scan_guard = self.try_start_scans(folders.iter().map(|folder| folder.folder.id))?;
        self.scan_all_inner(folders, on_progress).await
    }

    #[allow(clippy::cast_precision_loss)]
    async fn scan_all_inner(
        &self,
        folders: Vec<VirtualFolderWithPaths>,
        on_progress: Option<&(dyn Fn(f64) + Send + Sync)>,
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        let total = folders.len();
        let concurrency = total.min(default_library_concurrency()).max(1);
        tracing::info!(library_count = total, concurrency, "library scan started");
        let mut summary = LibraryScanSummary::default();
        if let Some(on_progress) = on_progress {
            on_progress(if total == 0 { 90.0 } else { 1.0 });
        }
        let scans = futures_util::stream::iter(folders)
            .map(|folder| async move {
                let library_id = folder.folder.id;
                let library_name = folder.folder.name.clone();
                tracing::info!(
                    %library_id,
                    %library_name,
                    path_count = folder.paths.len(),
                    "scanning library",
                );
                let mut summary = LibraryScanSummary::default();
                self.scan_one_folder(folder, &mut summary).await?;
                tracing::info!(
                    %library_id,
                    %library_name,
                    folders_seen = summary.folders_seen,
                    items_seen = summary.items_seen,
                    items_added = summary.items_added,
                    items_removed = summary.items_removed,
                    "library scan completed",
                );
                Ok::<_, LibraryScanError>(summary)
            })
            .buffer_unordered(concurrency);
        futures_util::pin_mut!(scans);
        let mut completed = 0;
        while let Some(library_summary) = scans.next().await {
            merge_scan_summary(&mut summary, library_summary?);
            completed += 1;
            if let Some(on_progress) = on_progress {
                on_progress(completed as f64 / total as f64 * 90.0);
            }
        }
        if let Some(on_progress) = on_progress {
            on_progress(95.0);
        }
        if let Err(error) = self.values.clear_inherited_tags().await {
            tracing::debug!(%error, "post-scan inherited-tags cleanup failed");
        }
        if let Some(on_progress) = on_progress {
            on_progress(100.0);
        }
        tracing::info!(
            folders_seen = summary.folders_seen,
            items_seen = summary.items_seen,
            items_added = summary.items_added,
            items_removed = summary.items_removed,
            "library scan completed",
        );
        Ok(summary)
    }

    /// Scans a single library collection.
    ///
    /// # Errors
    ///
    /// Returns an error when the collection scan fails.
    pub async fn scan_collection(
        &self,
        collection_id: Uuid,
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        let _scan_guard = self.try_start_scans([collection_id])?;
        self.scan_collection_inner(collection_id).await
    }

    async fn scan_collection_inner(
        &self,
        collection_id: Uuid,
    ) -> Result<LibraryScanSummary, LibraryScanError> {
        self.items.ensure_user_root().await?;
        let mut summary = LibraryScanSummary::default();
        let folders = self.folders.list().await?;
        if let Some(folder) = folders.into_iter().find(|f| f.folder.id == collection_id) {
            self.scan_one_folder(folder, &mut summary).await?;
        }
        if let Err(error) = self.values.clear_inherited_tags().await {
            tracing::debug!(%error, "post-scan inherited-tags cleanup failed");
        }
        Ok(summary)
    }

    async fn scan_one_folder(
        &self,
        mut folder: VirtualFolderWithPaths,
        summary: &mut LibraryScanSummary,
    ) -> Result<(), LibraryScanError> {
        let kind = ScanLibraryKind::from_collection_type(folder.folder.collection_type.as_deref());
        let collection = self.ensure_collection_folder(&mut folder).await?;
        summary.folders_seen += 1;
        let enabled = bool_option(&folder.folder.library_options, "Enabled", true);
        let allow_photos = collection_allows_photos(
            folder.folder.collection_type.as_deref(),
            &folder.folder.library_options,
        );
        let mut seen_paths = HashSet::new();
        let mut readable_roots = Vec::new();
        if !enabled {
            return Ok(());
        }
        for path in &folder.paths {
            let root = Path::new(&path.normalized_path);
            let readable = if kind.is_music() {
                self.scan_music_path(
                    root,
                    collection.id,
                    kind,
                    allow_photos,
                    summary,
                    &mut seen_paths,
                )
                .await?;
                true
            } else {
                self.scan_path(
                    root,
                    collection.id,
                    kind,
                    allow_photos,
                    summary,
                    &mut seen_paths,
                )
                .await?
            };
            if readable {
                readable_roots.push(root.to_path_buf());
            }
        }
        if kind.is_tv() {
            self.reconcile_series_hierarchy(collection.id).await?;
        }
        let removed = self
            .remove_stale_media(collection.id, kind, &seen_paths, &readable_roots)
            .await?;
        summary.items_removed += removed.len();
        summary.removed_ids.extend(removed);
        if kind.is_tv() {
            self.reconcile_series_hierarchy(collection.id).await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn reconcile_series_hierarchy(
        &self,
        collection_id: Uuid,
    ) -> Result<(), LibraryScanError> {
        let mut items_by_series = HashMap::<Uuid, Vec<base_item::Model>>::new();
        let mut series_order = Vec::new();
        for entry in self.items.descendants(collection_id).await? {
            let item = entry.item;
            if item.item_type == "Series" {
                series_order.push(item.id);
                items_by_series.entry(item.id).or_default().push(item);
            } else if let Some(series_id) = item.series_id {
                items_by_series.entry(series_id).or_default().push(item);
            }
        }
        for series_id in series_order {
            let mut seasons = Vec::new();
            let mut episodes = Vec::new();
            if let Some(items) = items_by_series.remove(&series_id) {
                for item in items {
                    match item.item_type.as_str() {
                        "Season" => seasons.push(item),
                        "Episode" => episodes.push(item),
                        _ => {}
                    }
                }
            }
            let mut episode_season_ids = episodes
                .iter()
                .filter_map(|episode| episode.season_id)
                .collect::<HashSet<_>>();

            for mut episode in episodes {
                if episode.season_id.is_some() {
                    continue;
                }
                let season_number = episode
                    .parent_index_number
                    .or_else(|| episode.index_number.map(|_| 1));
                let Some(season_number) = season_number else {
                    continue;
                };
                let season_key = format!("{}_{}", series_id.simple(), season_number);
                let season_id = stable_item_id(&season_key, "Season");
                let season_id = if let Some(season) = seasons
                    .iter()
                    .find(|season| season.index_number == Some(season_number))
                {
                    season.id
                } else if let Some(season) = self.items.get(season_id).await? {
                    let season_id = season.id;
                    seasons.push(season);
                    season_id
                } else {
                    let created = NewBaseItem {
                        id: season_id,
                        item_type: "Season".to_owned(),
                        data: Some(json!({ "CollectionType": "tvshows" })),
                        path: None,
                        parent_id: Some(series_id),
                        name: Some(format!("Season {season_number}")),
                        sort_name: Some(format!("Season {season_number}")),
                        media_type: None,
                        overview: None,
                        official_rating: None,
                        index_number: Some(season_number),
                        parent_index_number: None,
                        production_year: None,
                        premiere_date: None,
                        runtime_ticks: None,
                        is_folder: true,
                        is_virtual_item: false,
                        presentation_unique_key: Some(season_key),
                        primary_version_id: None,
                        series_id: Some(series_id),
                        season_id: None,
                        series_presentation_unique_key: Some(series_id.simple().to_string()),
                    };
                    let created = self.items.create(created).await?;
                    let season_id = created.id;
                    seasons.push(created);
                    season_id
                };
                let mut episode_changed = false;
                if episode.parent_id != Some(season_id) {
                    episode.parent_id = Some(season_id);
                    episode_changed = true;
                }
                if episode.season_id != Some(season_id) {
                    episode.season_id = Some(season_id);
                    episode_changed = true;
                }
                if episode.series_presentation_unique_key.as_deref()
                    != Some(series_id.simple().to_string().as_str())
                {
                    episode.series_presentation_unique_key = Some(series_id.simple().to_string());
                    episode_season_ids.insert(season_id);
                }
                if episode_changed {
                    self.items.update(episode).await?;
                    episode_season_ids.insert(season_id);
                }
            }

            for season in seasons {
                if season.path.is_none()
                    && !episode_season_ids.contains(&season.id)
                    && let Some(existing) = self.items.get(season.id).await?
                    && existing.path.is_none()
                {
                    self.items.delete(existing.id).await?;
                }
            }
        }
        Ok(())
    }

    async fn remove_stale_media(
        &self,
        parent_id: Uuid,
        kind: ScanLibraryKind,
        seen_paths: &HashSet<String>,
        readable_roots: &[PathBuf],
    ) -> Result<Vec<Uuid>, LibraryScanError> {
        if readable_roots.is_empty() {
            return Ok(Vec::new());
        }
        let descendants = self.items.descendants(parent_id).await?;
        let stale_ids = descendants
            .iter()
            .map(|entry| &entry.item)
            .filter(|item| match kind {
                ScanLibraryKind::TvShows => {
                    matches!(item.item_type.as_str(), "Episode" | "Trailer" | "Video")
                }
                ScanLibraryKind::Music => matches!(
                    item.item_type.as_str(),
                    "MusicArtist" | "MusicAlbum" | "Audio"
                ),
                ScanLibraryKind::Movies
                | ScanLibraryKind::MusicVideos
                | ScanLibraryKind::Generic => {
                    matches!(
                        item.item_type.as_str(),
                        "Movie" | "MusicVideo" | "Video" | "Trailer" | "Photo" | "Book" | "Audio"
                    )
                }
            })
            .filter_map(|item| {
                let path = item.path.as_deref()?;
                let stale = !seen_paths.contains(path)
                    && readable_roots
                        .iter()
                        .any(|root| Path::new(path).starts_with(root));
                stale.then_some(item.id)
            })
            .collect::<Vec<_>>();
        if !stale_ids.is_empty() {
            self.items.delete_many(&stale_ids).await?;
        }

        if kind.is_tv() {
            let descendants = self.items.descendants(parent_id).await?;
            let stale_seasons = descendants
                .iter()
                .map(|entry| &entry.item)
                .filter(|item| item.item_type == "Season" && item.path.is_some())
                .filter(|season| {
                    !descendants.iter().any(|entry| {
                        (entry.item.item_type == "Episode"
                            && entry.item.season_id == Some(season.id))
                            || entry.item.parent_id == Some(season.id)
                    })
                })
                .filter(|season| {
                    season.path.as_deref().is_some_and(|path| {
                        readable_roots
                            .iter()
                            .any(|root| Path::new(path).starts_with(root))
                    })
                })
                .map(|season| season.id)
                .collect::<Vec<_>>();
            if !stale_seasons.is_empty() {
                self.items.delete_many(&stale_seasons).await?;
                return Ok(stale_ids
                    .into_iter()
                    .chain(stale_seasons)
                    .collect::<Vec<_>>());
            }
        }

        Ok(stale_ids)
    }

    async fn scan_path(
        &self,
        root: &Path,
        parent_id: Uuid,
        kind: ScanLibraryKind,
        allow_photos: bool,
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
            allow_photos,
            summary,
            seen_paths,
            root,
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
                allow_photos,
                summary,
                seen_paths,
                root,
            )
            .await?;
        }
        Ok(true)
    }

    #[allow(clippy::too_many_lines)]
    async fn scan_music_path(
        &self,
        root: &Path,
        parent_id: Uuid,
        kind: ScanLibraryKind,
        allow_photos: bool,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
    ) -> Result<(), LibraryScanError> {
        let resolver = LibraryResolverChain::default_music_chain();
        let reader = FilesystemDirectoryReader;
        let ignore_rule = CoreResolutionIgnoreRule::new(NamingOptions::default(), "");
        let mut stack = vec![MusicScanEntry {
            path: root.to_path_buf(),
            parent_id,
            parent_kind: LibraryParentKind::Folder,
            parent_path: None,
            is_root: true,
        }];

        while let Some(directory) = stack.pop() {
            let mut entries = match fs::read_dir(&directory.path).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let mut children = Vec::new();
            let mut subdirectories = Vec::new();
            let mut files = Vec::new();
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
                        children.push(candidate);
                        subdirectories.push(path);
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
                if media_kind == MediaKind::Photo && !allow_photos {
                    continue;
                }
                summary.items_seen += 1;
                if let Some(path) = path.to_str() {
                    seen_paths.insert(path.to_owned());
                }
                children.push(candidate);
                files.push((path, media_kind));
            }

            let directory_path = directory.path.to_string_lossy().into_owned();
            if let Some(path) = directory.path.to_str() {
                seen_paths.insert(path.to_owned());
            }

            let resolved_item = if directory.is_root {
                None
            } else {
                resolver.resolve(&LibraryResolveArgs {
                    collection_type: Some(CollectionType::Music),
                    path: &directory_path,
                    is_directory: true,
                    children,
                    parent: directory.parent_kind,
                    parent_is_root: directory.is_root,
                    parent_path: directory.parent_path.as_deref(),
                    directory_reader: &reader,
                })
            };

            let (directory_id, child_parent_kind) = match resolved_item {
                Some(item) => match item.kind {
                    ResolvedLibraryItemKind::MusicArtist => (
                        self.ensure_music_folder(
                            &directory_path,
                            "MusicArtist",
                            directory.parent_id,
                            summary,
                        )
                        .await?,
                        LibraryParentKind::MusicArtist,
                    ),
                    ResolvedLibraryItemKind::MusicAlbum => (
                        self.ensure_music_folder(
                            &directory_path,
                            "MusicAlbum",
                            directory.parent_id,
                            summary,
                        )
                        .await?,
                        LibraryParentKind::MusicAlbum,
                    ),
                    _ => (directory.parent_id, LibraryParentKind::Folder),
                },
                None => (directory.parent_id, LibraryParentKind::Folder),
            };

            let child_parent_path = Arc::<str>::from(directory_path);
            for subdirectory in subdirectories {
                stack.push(MusicScanEntry {
                    path: subdirectory,
                    parent_id: directory_id,
                    parent_kind: child_parent_kind,
                    parent_path: Some(Arc::clone(&child_parent_path)),
                    is_root: false,
                });
            }

            let paths = files
                .iter()
                .filter_map(|(path, _)| path.to_str().map(String::from))
                .collect::<Vec<_>>();
            let existing = self.items.by_paths(&paths).await?;
            let existing_by_path = existing
                .into_iter()
                .filter_map(|item| Some((item.path.as_deref()?.to_owned(), item)))
                .collect::<HashMap<_, _>>();
            let extra_entries = extra_entries_for_files(&files);
            let extra_paths = Self::extra_paths_for_resolver_entries(&extra_entries, root)?;
            let regular_files = files
                .into_iter()
                .filter(|(path, _)| !extra_paths.contains(path.to_string_lossy().as_ref()))
                .collect::<Vec<_>>();
            summary.items_added += self
                .process_files(
                    &regular_files,
                    directory_id,
                    kind,
                    root,
                    existing_by_path,
                    summary,
                )
                .await?;
            self.ensure_extras(&extra_entries, kind, summary, seen_paths, root)
                .await?;
        }
        Ok(())
    }

    async fn ensure_music_folder(
        &self,
        path: &str,
        item_type: &str,
        parent_id: Uuid,
        summary: &mut LibraryScanSummary,
    ) -> Result<Uuid, LibraryScanError> {
        let item_id = stable_item_id(path, item_type);
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path)
            .to_owned();
        if let Some(mut existing) = self.items.get(item_id).await? {
            let mut changed = false;
            if existing.item_type != item_type {
                existing.item_type = item_type.to_owned();
                changed = true;
            }
            if existing.parent_id != Some(parent_id) {
                existing.parent_id = Some(parent_id);
                changed = true;
            }
            if existing.name.as_deref() != Some(name.as_str())
                || existing.sort_name.as_deref() != Some(name.as_str())
            {
                existing.name = Some(name.clone());
                existing.sort_name = Some(name);
                changed = true;
            }
            if !existing.is_folder {
                existing.is_folder = true;
                changed = true;
            }
            if existing.is_virtual_item {
                existing.is_virtual_item = false;
                changed = true;
            }
            if existing.presentation_unique_key.as_deref() != Some(path) {
                existing.presentation_unique_key = Some(path.to_owned());
                changed = true;
            }
            if changed {
                self.items.update(existing).await?;
            }
            return Ok(item_id);
        }

        let mut item = NewBaseItem::new(item_id, item_type);
        item.path = Some(path.to_owned());
        item.parent_id = Some(parent_id);
        item.name = Some(name.clone());
        item.sort_name = Some(name);
        item.is_folder = true;
        item.is_virtual_item = false;
        item.presentation_unique_key = Some(path.to_owned());
        item.data = Some(json!({ "CollectionType": "music" }));
        self.items.create(item).await?;
        summary.items_added += 1;
        summary.added_ids.push(item_id);
        Ok(item_id)
    }

    #[allow(clippy::too_many_arguments)]
    async fn scan_entries(
        &self,
        entries: &mut tokio::fs::ReadDir,
        pending: &mut Vec<PathBuf>,
        parent_id: Uuid,
        kind: ScanLibraryKind,
        allow_photos: bool,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
        library_root: &Path,
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
            if media_kind == MediaKind::Photo && !allow_photos {
                continue;
            }
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
        let existing_by_path = existing
            .into_iter()
            .filter_map(|item| Some((item.path.as_deref()?.to_owned(), item)))
            .collect::<HashMap<_, _>>();

        let extra_entries = extra_entries_for_files(&files);
        let extra_paths = Self::extra_paths_for_resolver_entries(&extra_entries, library_root)?;
        let regular_files = files
            .into_iter()
            .filter(|(path, _)| !extra_paths.contains(path.to_string_lossy().as_ref()))
            .collect::<Vec<_>>();
        summary.items_added += self
            .process_files(
                &regular_files,
                parent_id,
                kind,
                library_root,
                existing_by_path,
                summary,
            )
            .await?;
        self.group_scanned_video_entries(&regular_files, kind, library_root)
            .await?;
        self.ensure_extras(&extra_entries, kind, summary, seen_paths, library_root)
            .await?;
        Ok(())
    }

    async fn group_scanned_video_entries(
        &self,
        files: &[(PathBuf, MediaKind)],
        kind: ScanLibraryKind,
        library_root: &Path,
    ) -> Result<(), LibraryScanError> {
        let paths = files
            .iter()
            .filter(|(_, media_kind)| *media_kind == MediaKind::Video)
            .filter_map(|(path, _)| path.to_str().map(String::from))
            .collect::<Vec<_>>();
        if paths.len() < 2 {
            return Ok(());
        }
        let items = self.items.by_paths(&paths).await?;
        if items.len() < 2 {
            return Ok(());
        }
        let mut by_path = items
            .into_iter()
            .filter_map(|item| Some((item.path.as_deref()?.to_owned(), item)))
            .collect::<HashMap<_, _>>();
        let options = NamingOptions::default();
        let videos = paths
            .iter()
            .filter_map(|path| {
                VideoResolver::resolve_file_with_library_root(
                    Some(path),
                    &options,
                    library_root.to_str(),
                )
            })
            .collect::<Vec<_>>();
        let collection_type = if kind.is_tv() {
            Some(jellyfin_naming::video_list::CollectionType::TvShows)
        } else {
            Some(jellyfin_naming::video_list::CollectionType::Movies)
        };
        let groups = VideoListResolver::new(options).resolve_owned_with_options(
            videos,
            true,
            collection_type,
        );
        for group in groups {
            let Some(primary_path) = group.files.first().map(|file| file.path.as_str()) else {
                continue;
            };
            let Some(primary) = by_path.remove(primary_path) else {
                continue;
            };
            let primary_id = primary.id;
            let original_primary_version_id = primary.primary_version_id;
            let mut updated_primary = primary;
            updated_primary.primary_version_id = None;
            let mut primary_changed =
                updated_primary.primary_version_id != original_primary_version_id;
            if group.files.len() > 1 {
                let parts = group
                    .files
                    .iter()
                    .skip(1)
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>();
                primary_changed |= set_additional_parts(&mut updated_primary, &parts);
            }
            if !group.name.is_empty()
                && (updated_primary.name.as_deref() != Some(group.name.as_str())
                    || updated_primary.sort_name.as_deref() != Some(group.name.as_str()))
            {
                updated_primary.name = Some(group.name.clone());
                updated_primary.sort_name = Some(group.name);
                primary_changed = true;
            }
            if primary_changed {
                self.items.update(updated_primary).await?;
            }

            let mut version_paths = group
                .files
                .iter()
                .skip(1)
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>();
            for alternate in &group.alternate_versions {
                version_paths.extend(alternate.files.iter().map(|file| file.path.as_str()));
            }
            for path in version_paths {
                let Some(mut item) = by_path.remove(path) else {
                    continue;
                };
                if item.primary_version_id == Some(primary_id) {
                    continue;
                }
                item.primary_version_id = Some(primary_id);
                self.items.update(item).await?;
            }
        }
        Ok(())
    }

    fn extra_paths_for_resolver_entries(
        entries: &[ExtraFileSystemEntry],
        library_root: &Path,
    ) -> Result<HashSet<String>, LibraryScanError> {
        let options = NamingOptions::default();
        let eligible_entries = entries
            .iter()
            .filter(|entry| {
                ExtraResolver::resolve(&entry.full_name, &options)
                    .extra_type
                    .is_none()
            })
            .collect::<Vec<_>>();
        let resolver =
            LibraryExtrasResolver::with_library_root(options, library_root.to_string_lossy());
        let reader = TokioExtraDirectoryReader;
        let mut extra_paths = HashSet::new();
        for entry in eligible_entries {
            let owner = ExtraOwner::new(
                entry.full_name.as_str(),
                display_name(&entry.full_name),
                ExtraOwnerKind::Movie,
            );
            let extras = resolver
                .find_extras(&owner, entries, &reader)
                .map_err(LibraryScanError::Io)?;
            for extra in extras {
                extra_paths.insert(extra.path);
            }
        }
        Ok(extra_paths)
    }

    async fn ensure_extras(
        &self,
        entries: &[ExtraFileSystemEntry],
        kind: ScanLibraryKind,
        summary: &mut LibraryScanSummary,
        seen_paths: &mut HashSet<String>,
        library_root: &Path,
    ) -> Result<(), LibraryScanError> {
        if kind.is_tv() {
            return Ok(());
        }
        let options = NamingOptions::default();
        let eligible_entries = entries
            .iter()
            .filter(|entry| {
                ExtraResolver::resolve(&entry.full_name, &options)
                    .extra_type
                    .is_none()
            })
            .collect::<Vec<_>>();
        let resolver =
            LibraryExtrasResolver::with_library_root(options, library_root.to_string_lossy());
        let reader = TokioExtraDirectoryReader;
        for entry in eligible_entries {
            let owner = ExtraOwner::new(
                entry.full_name.as_str(),
                display_name(&entry.full_name),
                ExtraOwnerKind::Movie,
            );
            let extras = resolver
                .find_extras(&owner, entries, &reader)
                .map_err(LibraryScanError::Io)?;
            for extra in extras {
                let owner_id = self
                    .items
                    .by_paths(std::slice::from_ref(&entry.full_name))
                    .await?
                    .into_iter()
                    .next()
                    .map(|item| item.id);
                let path = if let Some(owner_id) = owner_id {
                    self.ensure_extra_item(extra, owner_id, summary).await?
                } else {
                    extra.path
                };
                seen_paths.insert(path);
            }
        }
        Ok(())
    }

    async fn ensure_extra_item(
        &self,
        extra: ResolvedLibraryExtra,
        owner_id: Uuid,
        summary: &mut LibraryScanSummary,
    ) -> Result<String, LibraryScanError> {
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
            let mut updated = existing;
            updated.item_type = item_type.to_owned();
            updated.parent_id = Some(owner_id);
            updated.data = Some(serde_json::Value::Object(data));
            self.items.update(updated).await?;
            return Ok(extra.path);
        }
        let path = extra.path;
        let media_type = match extra.media_kind {
            ExtraMediaKind::Audio => "Audio",
            _ => "Video",
        };
        let mut item = NewBaseItem::new(stable_id, item_type);
        item.path = Some(path.clone());
        item.parent_id = Some(owner_id);
        let name = extra.name;
        item.name = Some(name.clone());
        item.sort_name = Some(name);
        item.media_type = Some(media_type.to_owned());
        item.is_folder = false;
        item.is_virtual_item = false;
        item.presentation_unique_key = Some(path);
        item.data = Some(serde_json::Value::Object(data));
        let mut created = self.items.create(item).await?;
        summary.items_added += 1;
        Ok(created
            .path
            .take()
            .expect("new extra item always persists its path"))
    }

    async fn process_files(
        &self,
        files: &[(PathBuf, MediaKind)],
        parent_id: Uuid,
        kind: ScanLibraryKind,
        library_root: &Path,
        mut existing_by_path: HashMap<String, base_item::Model>,
        summary: &mut LibraryScanSummary,
    ) -> Result<usize, LibraryScanError> {
        if files.is_empty() {
            return Ok(0);
        }
        let concurrency = self.fanout_concurrency();
        if concurrency <= 1 {
            let mut added = 0;
            for (path, media_kind) in files {
                let existing = path.to_str().and_then(|p| existing_by_path.remove(p));
                let (item_added, item_id) = self
                    .ensure_media_item_with_permit(
                        path,
                        parent_id,
                        *media_kind,
                        kind,
                        library_root,
                        existing,
                    )
                    .await?;
                if item_added {
                    added += 1;
                    summary.added_ids.push(item_id);
                }
                if path.to_str().is_some() {
                    summary.changed_ids.push(item_id);
                }
            }
            return Ok(added);
        }
        let mut files = files.iter();
        let work = FuturesUnordered::new();
        for (path, media_kind) in files.by_ref().take(concurrency) {
            let existing = path
                .to_str()
                .and_then(|value| existing_by_path.remove(value));
            work.push(self.ensure_media_item_with_permit(
                path,
                parent_id,
                *media_kind,
                kind,
                library_root,
                existing,
            ));
        }
        let mut work = work;
        let mut added = 0;
        while let Some(result) = work.next().await {
            if let Some((path, media_kind)) = files.next() {
                let existing = path
                    .to_str()
                    .and_then(|value| existing_by_path.remove(value));
                work.push(self.ensure_media_item_with_permit(
                    path,
                    parent_id,
                    *media_kind,
                    kind,
                    library_root,
                    existing,
                ));
            }
            match result {
                Ok((true, item_id)) => {
                    added += 1;
                    summary.added_ids.push(item_id);
                    summary.changed_ids.push(item_id);
                }
                Ok((false, item_id)) => summary.changed_ids.push(item_id),
                Err(error) => {
                    tracing::debug!(%error, "concurrent media item processing failed");
                }
            }
        }
        Ok(added)
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_media_item_with_permit(
        &self,
        path: &Path,
        parent_id: Uuid,
        media_kind: MediaKind,
        kind: ScanLibraryKind,
        library_root: &Path,
        existing: Option<base_item::Model>,
    ) -> Result<(bool, Uuid), LibraryScanError> {
        let _permit = self.media_item_limiter.acquire().await;
        self.ensure_media_item(path, parent_id, media_kind, kind, library_root, existing)
            .await
    }

    async fn ensure_collection_folder(
        &self,
        folder: &mut jellyfin_data::VirtualFolderWithPaths,
    ) -> Result<jellyfin_data::entities::base_item::Model, LibraryScanError> {
        let name = std::mem::take(&mut folder.folder.name);
        if let Some(mut item) = self.items.get(folder.folder.id).await? {
            "CollectionFolder".clone_into(&mut item.item_type);
            item.parent_id = Some(USER_ROOT_FOLDER_ID);
            // ALLOW: persisted name and sort name are independent owned fields.
            item.name = Some(name.clone());
            item.sort_name = Some(name);
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
        // ALLOW: persisted name and sort name are independent owned fields.
        item.name = Some(name.clone());
        item.sort_name = Some(name);
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
        kind: ScanLibraryKind,
        library_root: &Path,
        existing: Option<base_item::Model>,
    ) -> Result<(bool, Uuid), LibraryScanError> {
        let Some(path_str) = path.to_str() else {
            return Ok((false, Uuid::nil()));
        };
        let is_strm = is_strm_path(path);
        let strm_target = if is_strm {
            read_strm_target(path).await?
        } else {
            None
        };
        let media_source_path = strm_target.as_deref().unwrap_or(path_str);
        if let Some(mut existing) = existing {
            if !kind.is_tv() {
                let existing_id = existing.id;
                let desired_type = if media_kind == MediaKind::Video {
                    kind.video_item_type()
                } else {
                    media_kind.item_type()
                };
                let mut changed = false;
                if existing.parent_id != Some(parent_id) {
                    existing.parent_id = Some(parent_id);
                    changed = true;
                }
                if existing.item_type != desired_type {
                    existing.item_type = desired_type.to_owned();
                    changed = true;
                }
                if is_strm
                    && apply_strm_metadata(&mut existing, media_source_path, strm_target.as_deref())
                {
                    changed = true;
                }
                if media_kind.needs_probe()
                    && let Some(mut media_info) = self
                        .ensure_media_streams(
                            existing.id,
                            media_source_path,
                            path_str,
                            media_kind,
                            !is_strm,
                        )
                        .await?
                    && apply_probed_item_metadata(&mut existing, media_source_path, &mut media_info)
                {
                    changed = true;
                }
                if apply_nfo_metadata(&mut existing, path_str) {
                    changed = true;
                }
                self.persist_scan_relations(existing.id, path_str, &existing.item_type, None)
                    .await?;
                self.discover_local_images(existing.id, path_str).await?;
                if changed {
                    self.items.update(existing).await?;
                }
                return Ok((false, existing_id));
            }
            return self
                .ensure_episode_item(
                    Some(existing),
                    path,
                    parent_id,
                    library_root,
                    media_kind,
                    path_str,
                    strm_target.as_deref(),
                )
                .await;
        }

        if kind.is_tv() && media_kind == MediaKind::Video {
            return self
                .ensure_episode_item(
                    None,
                    path,
                    parent_id,
                    library_root,
                    media_kind,
                    path_str,
                    strm_target.as_deref(),
                )
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
        item.data = Some(media_item_data_with_strm(
            media_source_path,
            strm_target.as_deref(),
        ));
        let mut item = self.items.create(item).await?;
        if media_kind.needs_probe()
            && let Some(mut media_info) = self
                .ensure_media_streams(item.id, media_source_path, path_str, media_kind, !is_strm)
                .await?
            && apply_probed_item_metadata(&mut item, media_source_path, &mut media_info)
        {
            item = self.items.update(item).await?;
        }
        if apply_nfo_metadata(&mut item, path_str) {
            item = self.items.update(item).await?;
        }
        self.persist_scan_relations(item.id, path_str, &item.item_type, None)
            .await?;
        self.discover_local_images(item.id, path_str).await?;
        Ok((true, item.id))
    }

    #[allow(clippy::too_many_lines)]
    async fn ensure_episode_item(
        &self,
        existing: Option<base_item::Model>,
        path: &Path,
        parent_id: Uuid,
        library_root: &Path,
        media_kind: MediaKind,
        path_str: &str,
        strm_target: Option<&str>,
    ) -> Result<(bool, Uuid), LibraryScanError> {
        let is_strm = is_strm_path(path);
        let media_source_path = strm_target.unwrap_or(path_str);
        let ep_result = crate::episode_parser::parse_episode(path);
        let season_number = ep_result.season_number;
        let episode_number = ep_result.episode_number;
        let series_name = ep_result.series_name;

        let mut series_id = None;
        let mut season_id = None;
        let mut series_puk = None;
        if let Some((series_name, series_path)) =
            episode_series_context(path, library_root, series_name)
        {
            let (series_item_id, _) = self
                .ensure_series_item(&series_name, parent_id, series_path.as_deref(), path)
                .await?;
            self.persist_scan_relations(series_item_id, path_str, "Series", None)
                .await?;
            series_id = Some(series_item_id);
            series_puk = Some(series_item_id.simple().to_string());

            if let Some(sn) = season_number {
                let season_key = format!("{}_{}", series_item_id.simple(), sn);
                let season_item_id = stable_item_id(&season_key, "Season");
                if self.items.get(season_item_id).await?.is_none() {
                    let season_path = path.parent().and_then(|folder| {
                        let name = folder.file_name()?.to_str()?;
                        parse_season_directory(name)
                            .filter(|number| *number == sn)
                            .map(|_| folder.to_path_buf())
                    });
                    let mut season = NewBaseItem::new(season_item_id, "Season");
                    season.path = season_path.map(|path| path.to_string_lossy().into_owned());
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
                self.persist_scan_relations(season_item_id, path_str, "Season", Some(sn))
                    .await?;
                season_id = Some(season_item_id);
            }
        }

        let item_type = "Episode";
        let name = display_name(path_str);
        let stable_id = stable_item_id(path_str, item_type);

        if let Some(mut existing) = existing {
            let existing_id = existing.id;
            existing.parent_id = season_id.or(series_id).or(Some(parent_id));
            item_type.clone_into(&mut existing.item_type);
            existing.index_number = episode_number;
            existing.parent_index_number = season_number;
            if let Some(sid) = series_id {
                existing.series_id = Some(sid);
            }
            if let Some(sid) = season_id {
                existing.season_id = Some(sid);
            }
            existing.series_presentation_unique_key = series_puk;
            let strm_changed =
                is_strm && apply_strm_metadata(&mut existing, media_source_path, strm_target);
            let nfo_changed = apply_episode_nfo_metadata(&mut existing, path);
            self.persist_scan_relations(existing.id, path_str, &existing.item_type, season_number)
                .await?;
            if let Some(mut media_info) = self
                .ensure_media_streams(
                    existing.id,
                    media_source_path,
                    path_str,
                    media_kind,
                    !is_strm,
                )
                .await?
                && apply_probed_item_metadata(&mut existing, media_source_path, &mut media_info)
            {
                self.items.update(existing).await?;
            } else if nfo_changed || strm_changed {
                self.items.update(existing).await?;
            }
            return Ok((false, existing_id));
        }

        let mut item = NewBaseItem::new(stable_id, item_type);
        item.path = Some(path_str.to_owned());
        item.parent_id = season_id.or(series_id).or(Some(parent_id));
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
        item.data = Some(media_item_data_with_strm(media_source_path, strm_target));
        let mut item = self.items.create(item).await?;
        if apply_episode_nfo_metadata(&mut item, path) {
            item = self.items.update(item).await?;
        }
        self.persist_scan_relations(item.id, path_str, &item.item_type, season_number)
            .await?;
        if let Some(mut media_info) = self
            .ensure_media_streams(item.id, media_source_path, path_str, media_kind, !is_strm)
            .await?
            && apply_probed_item_metadata(&mut item, media_source_path, &mut media_info)
        {
            let item_id = item.id;
            self.items.update(item).await?;
            return Ok((true, item_id));
        }

        let item_id = item.id;
        Ok((true, item_id))
    }

    async fn ensure_series_item(
        &self,
        name: &str,
        parent_id: Uuid,
        series_path: Option<&Path>,
        episode_path: &Path,
    ) -> Result<(Uuid, bool), LibraryScanError> {
        let path_id = series_path.map(|path| stable_item_id(&path.to_string_lossy(), "Series"));
        let name_id = stable_item_id(name, "Series");

        let mut existing = None;
        if let Some(path_id) = path_id
            && let Some(item) = self.items.get(path_id).await?
        {
            existing = Some(item);
        } else if let Some(item) = self.items.get(name_id).await? {
            existing = Some(item);
        }

        if let Some(mut series) = existing {
            let mut changed = false;
            if series.item_type != "Series" {
                "Series".clone_into(&mut series.item_type);
                changed = true;
            }
            if let Some(path) = series_path
                && series.path.as_deref() != Some(path.to_string_lossy().as_ref())
            {
                series.path = Some(path.to_string_lossy().into_owned());
                changed = true;
            }
            if series.parent_id != Some(parent_id) {
                series.parent_id = Some(parent_id);
                changed = true;
            }
            if series.name.is_none() || series.sort_name.is_none() {
                series.name = Some(name.to_owned());
                series.sort_name = Some(name.to_owned());
                changed = true;
            }
            if !series.is_folder || series.is_virtual_item {
                series.is_folder = true;
                series.is_virtual_item = false;
                changed = true;
            }
            let presentation_key = series_path.map_or_else(
                || name.to_owned(),
                |path| path.to_string_lossy().into_owned(),
            );
            if series.presentation_unique_key.as_deref() != Some(presentation_key.as_str()) {
                series.presentation_unique_key = Some(presentation_key);
                changed = true;
            }
            if apply_series_nfo_metadata(&mut series, episode_path) {
                changed = true;
            }
            if changed {
                series = self.items.update(series).await?;
            }
            return Ok((series.id, false));
        }

        let id = path_id.unwrap_or(name_id);
        let mut series = NewBaseItem::new(id, "Series");
        series.path = series_path.map(|path| path.to_string_lossy().into_owned());
        series.parent_id = Some(parent_id);
        series.name = Some(name.to_owned());
        series.sort_name = Some(name.to_owned());
        series.is_folder = true;
        series.is_virtual_item = false;
        series.presentation_unique_key = Some(series_path.map_or_else(
            || name.to_owned(),
            |path| path.to_string_lossy().into_owned(),
        ));
        series.data = Some(json!({ "CollectionType": "tvshows" }));
        let mut series = self.items.create(series).await?;
        let mut series = if apply_series_nfo_metadata(&mut series, episode_path) {
            self.items.update(series).await?
        } else {
            series
        };
        series.is_folder = true;
        Ok((series.id, true))
    }

    async fn persist_scan_relations(
        &self,
        item_id: Uuid,
        path: &str,
        item_type: &str,
        season_number: Option<i32>,
    ) -> Result<(), LibraryScanError> {
        let Some(relations) = scan_nfo_relations(path, item_type, season_number) else {
            return Ok(());
        };
        if !relations.genres.is_empty() {
            self.updates
                .update(
                    item_id,
                    ItemMetadataPatch {
                        genres: Some(relations.genres),
                        ..ItemMetadataPatch::default()
                    },
                )
                .await?;
        }
        if !relations.tags.is_empty() {
            self.updates
                .update(
                    item_id,
                    ItemMetadataPatch {
                        tags: Some(relations.tags),
                        ..ItemMetadataPatch::default()
                    },
                )
                .await?;
        }
        for studio in relations.studios {
            self.values
                .link(item_id, ItemValueType::Studios, &studio)
                .await?;
        }
        for (list_order, person) in relations.people.into_iter().enumerate() {
            self.people
                .link(
                    item_id,
                    NewPerson::new(person.name),
                    &person.person_type,
                    Some(&person.role),
                    person.sort_order,
                    i32::try_from(list_order).unwrap_or(i32::MAX),
                )
                .await?;
        }
        Ok(())
    }

    async fn ensure_media_streams(
        &self,
        item_id: Uuid,
        media_source_path: &str,
        sidecar_path: &str,
        media_kind: MediaKind,
        should_probe: bool,
    ) -> Result<Option<MediaInfo>, LibraryScanError> {
        let existing = self
            .streams
            .query(MediaStreamQuery {
                item_id,
                stream_index: None,
                stream_type: None,
            })
            .await?;
        let default_stream = default_stream(media_source_path, media_kind);
        if !existing.is_empty() && (existing.len() != 1 || existing[0] != default_stream) {
            return Ok(None);
        }
        let mut media_info = if should_probe {
            self.probe_media_info(media_source_path, media_kind).await
        } else {
            None
        };
        let mut streams = Vec::new();
        if let Some(media_info) = media_info.as_mut() {
            let attachment_images = media_info
                .media_attachments
                .iter()
                .filter_map(|attachment| {
                    attachment_image_type(attachment)
                        .map(|image_type| (attachment.index, image_type))
                })
                .collect::<Vec<_>>();
            let video_stream_index = media_info
                .media_streams
                .iter()
                .find(|stream| stream.stream_type == MediaStreamType::Video)
                .map(|stream| stream.index);
            let probed_attachments = attachments_from_media_info(media_info);
            self.attachments
                .replace(item_id, &probed_attachments)
                .await?;
            self.chapters
                .replace(item_id, chapters_from_media_info(media_info))
                .await?;
            self.discover_embedded_images(
                item_id,
                media_source_path,
                &attachment_images,
                video_stream_index,
                media_info.runtime_ticks,
            )
            .await?;
            streams = streams_from_media_info(media_info);
        }
        if streams.is_empty() {
            streams.push(default_stream);
        }
        let external_subtitles = self
            .resolve_external_subtitle_streams(sidecar_path, next_stream_index(&streams))
            .await?;
        streams.extend(external_subtitles);
        self.streams.replace(item_id, &streams).await?;
        Ok(media_info)
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
        attachment_images: &[(i32, BaseItemImageType)],
        video_stream_index: Option<i32>,
        runtime_ticks: Option<i64>,
    ) -> Result<(), LibraryScanError> {
        let image_cache_directory = self.image_cache_directory();
        tokio::fs::create_dir_all(image_cache_directory.as_path()).await?;
        let mut has_primary = false;
        for image in self.images.list(item_id).await? {
            if image.image_type == BaseItemImageType::Primary {
                has_primary = true;
                break;
            }
        }

        for &(stream_index, image_type) in attachment_images {
            let output =
                image_cache_directory.join(format!("embedded-{item_id}-{stream_index}.jpg"));
            if self
                .extract_attachment_image(path, stream_index, &output)
                .await
            {
                self.persist_generated_image(item_id, image_type, &output)
                    .await?;
                if image_type == BaseItemImageType::Primary {
                    has_primary = true;
                }
            }
        }

        if !has_primary && let Some(video_stream_index) = video_stream_index {
            let offset_ticks = runtime_ticks
                .filter(|runtime| *runtime > 0)
                .map_or(10 * 10_000_000, |runtime| runtime / 10);
            let output = image_cache_directory
                .join(format!("screenshot-{item_id}-{video_stream_index}.jpg"));
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
        let ffmpeg_path = self.ffmpeg_path();
        let status = tokio::process::Command::new(ffmpeg_path.as_path())
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
        let ffmpeg_path = self.ffmpeg_path();
        let status = tokio::process::Command::new(ffmpeg_path.as_path())
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
        let probe_path = Arc::clone(&self.probe_path);
        let probe_input = path.to_owned();
        match tokio::task::spawn_blocking(move || {
            probe_media_info(&probe_path, &probe_input, media_kind)
        })
        .await
        {
            Ok(Ok(media_info)) => Some(media_info),
            Ok(Err(error)) => {
                tracing::debug!(path, error = %error, "media probe failed during library scan");
                None
            }
            Err(error) => {
                tracing::debug!(path, error = %error, "media probe task failed during library scan");
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
    available_parallelism().map_or(1, |n| n.get().saturating_sub(3).max(1))
}

fn default_library_concurrency() -> usize {
    available_parallelism().map_or(2, |n| n.get().clamp(2, 4))
}

fn merge_scan_summary(summary: &mut LibraryScanSummary, mut other: LibraryScanSummary) {
    summary.folders_seen += other.folders_seen;
    summary.items_added += other.items_added;
    summary.items_removed += other.items_removed;
    summary.items_seen += other.items_seen;
    summary.added_ids.append(&mut other.added_ids);
    summary.changed_ids.append(&mut other.changed_ids);
    summary.removed_ids.append(&mut other.removed_ids);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Audio,
    Video,
    Photo,
    Book,
}

struct MusicScanEntry {
    path: PathBuf,
    parent_id: Uuid,
    parent_kind: LibraryParentKind,
    parent_path: Option<Arc<str>>,
    is_root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanLibraryKind {
    Movies,
    TvShows,
    Music,
    MusicVideos,
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
            CollectionType::Music => Self::Music,
            CollectionType::MusicVideos => Self::MusicVideos,
            _ => Self::Generic,
        }
    }

    const fn is_tv(self) -> bool {
        matches!(self, Self::TvShows)
    }

    const fn is_music(self) -> bool {
        matches!(self, Self::Music)
    }

    const fn video_item_type(self) -> &'static str {
        match self {
            Self::Movies => "Movie",
            Self::MusicVideos => "MusicVideo",
            Self::TvShows | Self::Music | Self::Generic => "Video",
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

fn is_strm_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("strm"))
}

async fn read_strm_target(path: &Path) -> Result<Option<String>, std::io::Error> {
    let contents = fs::read_to_string(path).await?;
    Ok(contents.lines().find_map(|line| {
        let target = line.trim().trim_start_matches('\u{feff}').trim();
        (!target.is_empty()).then(|| target.to_owned())
    }))
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

fn episode_series_context(
    episode_path: &Path,
    library_root: &Path,
    parsed_name: Option<String>,
) -> Option<(String, Option<PathBuf>)> {
    let folder = episode_path.parent()?;
    if folder.starts_with(library_root)
        && folder
            .strip_prefix(library_root)
            .is_ok_and(|relative| relative.components().count() == 0)
    {
        return parsed_name.map(|name| (name, None));
    }
    let folder = if folder
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_season_directory)
        .is_some()
    {
        folder.parent().unwrap_or(folder)
    } else {
        folder
    };
    let name = folder
        .file_name()?
        .to_str()
        .map(str::to_owned)
        .map(|name| clean_series_name(&name))
        .or_else(|| parsed_name.filter(|name| !name.trim().is_empty()))?;
    Some((name, Some(folder.to_path_buf())))
}

fn clean_series_name(name: &str) -> String {
    name.trim().replace(['.', '_'], " ")
}

fn extra_entries_for_files(files: &[(PathBuf, MediaKind)]) -> Vec<ExtraFileSystemEntry> {
    files
        .iter()
        .filter(|(_, media_kind)| matches!(media_kind, MediaKind::Video | MediaKind::Audio))
        .map(|(path, _)| ExtraFileSystemEntry::new(path.to_string_lossy().into_owned(), false))
        .collect()
}

fn bool_option(options: &Value, key: &str, default: bool) -> bool {
    options.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn collection_allows_photos(collection_type: Option<&str>, options: &Value) -> bool {
    let parsed = collection_type
        .and_then(|value| CollectionType::from_str(value).ok())
        .unwrap_or(CollectionType::Unknown);
    match parsed {
        CollectionType::Photos => true,
        CollectionType::HomeVideos => bool_option(options, "EnablePhotos", false),
        _ => false,
    }
}

fn apply_probed_item_metadata(
    item: &mut jellyfin_data::entities::base_item::Model,
    path: &str,
    media_info: &mut MediaInfo,
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
        item.overview = media_info.overview.take();
        changed = true;
    }
    if let Some(sort_name) = media_info.forced_sort_name.as_ref()
        && item.sort_name.as_deref() != Some(sort_name)
    {
        item.sort_name = media_info.forced_sort_name.take();
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

struct ScanNfoRelations {
    genres: Vec<String>,
    tags: Vec<String>,
    studios: Vec<String>,
    people: Vec<ScanNfoPerson>,
}

struct ScanNfoPerson {
    name: String,
    person_type: String,
    role: String,
    sort_order: Option<i32>,
}

fn scan_nfo_relations(
    path: &str,
    item_type: &str,
    season_number: Option<i32>,
) -> Option<ScanNfoRelations> {
    match item_type {
        "Movie" | "Video" | "Trailer" | "MusicVideo" => {
            read_movie_nfo(path).map(relations_from_movie_nfo)
        }
        "Episode" => read_episode_nfo(path).map(relations_from_nfo_metadata),
        "Series" => read_series_nfo(path).map(relations_from_nfo_metadata),
        "Season" => read_season_nfo(path, season_number).map(relations_from_nfo_metadata),
        _ => None,
    }
}

fn read_movie_nfo(path: &str) -> Option<MovieNfo> {
    let nfo_path = movie_nfo_save_paths(&MovieNfoLocation {
        path: PathBuf::from(path),
        is_in_mixed_folder: false,
        video_type: MovieVideoType::File,
    })
    .into_iter()
    .find(|path| path.is_file())?;
    parse_movie_nfo_file(nfo_path).ok()
}

fn read_episode_nfo(path: &str) -> Option<NfoMetadata> {
    let nfo_path = Path::new(path).with_extension("nfo");
    let input = std::fs::read_to_string(nfo_path).ok()?;
    parse_nfo(&input, NfoDocumentKind::Episode).ok()
}

fn read_series_nfo(episode_path: &str) -> Option<NfoMetadata> {
    let directory = series_directory(Path::new(episode_path))?;
    let input = std::fs::read_to_string(directory.join("tvshow.nfo")).ok()?;
    parse_nfo(&input, NfoDocumentKind::Series).ok()
}

fn read_season_nfo(episode_path: &str, season_number: Option<i32>) -> Option<NfoMetadata> {
    let directory = season_directory(Path::new(episode_path), season_number)?;
    let candidate = season_nfo_path(&directory, season_number);
    let input = std::fs::read_to_string(candidate).ok()?;
    parse_nfo(&input, NfoDocumentKind::Season).ok()
}

fn relations_from_movie_nfo(movie: MovieNfo) -> ScanNfoRelations {
    let genres = movie.genres;
    let studios = movie.studios;
    ScanNfoRelations {
        genres,
        tags: Vec::new(),
        studios,
        people: movie.people.into_iter().map(scan_nfo_person).collect(),
    }
}

fn relations_from_nfo_metadata(metadata: NfoMetadata) -> ScanNfoRelations {
    let genres = metadata.genres;
    let tags = metadata.tags;
    let studios = metadata.studios;
    ScanNfoRelations {
        genres,
        tags,
        studios,
        people: metadata.people.into_iter().map(scan_nfo_person).collect(),
    }
}

fn scan_nfo_person(person: NfoPerson) -> ScanNfoPerson {
    let person_type = match &person.kind {
        NfoPersonKind::Director => "Director",
        NfoPersonKind::Writer => "Writer",
        NfoPersonKind::Lyricist => "Lyricist",
        NfoPersonKind::Other(kind) if !kind.trim().is_empty() => kind,
        NfoPersonKind::Actor | NfoPersonKind::Other(_) => "Actor",
    }
    .to_owned();
    ScanNfoPerson {
        name: person.name,
        person_type,
        role: person.role,
        sort_order: person.sort_order,
    }
}

#[allow(clippy::too_many_lines)]
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
    changed |= upsert_bool(&mut data, "IsLocked", nfo.is_locked);
    changed |= upsert_strings(&mut data, "LockedFields", &nfo.locked_fields);
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

#[allow(clippy::too_many_lines)]
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
        let season_number = season_number?;
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

fn media_item_data_with_strm(media_source_path: &str, strm_target: Option<&str>) -> Value {
    let mut data = media_item_data(media_source_path, None);
    if let Some(target) = strm_target {
        data.as_object_mut()
            .expect("media item data is always an object")
            .insert("StrmTarget".to_owned(), Value::String(target.to_owned()));
    }
    data
}

fn apply_strm_metadata(
    item: &mut base_item::Model,
    media_source_path: &str,
    strm_target: Option<&str>,
) -> bool {
    let mut data = merged_media_item_data(item.data.as_ref(), media_source_path, None);
    let object = data
        .as_object_mut()
        .expect("media item data is always an object");
    if let Some(target) = strm_target {
        object.insert("StrmTarget".to_owned(), Value::String(target.to_owned()));
    } else {
        object.remove("StrmTarget");
    }
    if item.data.as_ref() == Some(&data) {
        return false;
    }
    item.data = Some(data);
    true
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
    let container = media_info
        .and_then(|info| info.container.as_deref())
        .map(str::to_owned)
        .or_else(|| path_extension(path));
    object.insert(
        "Container".to_owned(),
        container.map_or(Value::Null, Value::String),
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
        color_range: None,
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

fn streams_from_media_info(media_info: &mut MediaInfo) -> Vec<PersistedMediaStream> {
    std::mem::take(&mut media_info.media_streams)
        .into_iter()
        .map(stream_from_probe)
        .collect()
}

fn attachments_from_media_info(media_info: &mut MediaInfo) -> Vec<PersistedMediaAttachment> {
    std::mem::take(&mut media_info.media_attachments)
        .into_iter()
        .map(attachment_from_probe)
        .collect()
}

fn chapters_from_media_info(media_info: &mut MediaInfo) -> Vec<NewChapter> {
    let runtime_ticks = media_info.runtime_ticks.unwrap_or_default();
    let mut chapters = std::mem::take(&mut media_info.chapters)
        .into_iter()
        .peekable();
    std::iter::from_fn(|| {
        let chapter = chapters.next()?;
        let end_position_ticks = chapters
            .peek()
            .map_or(runtime_ticks, |next| next.start_position_ticks);
        Some((chapter, end_position_ticks))
    })
    .enumerate()
    .map(|(index, (chapter, end_position_ticks))| NewChapter {
        index_number: i32::try_from(index).unwrap_or(i32::MAX),
        start_position_ticks: chapter.start_position_ticks,
        end_position_ticks: end_position_ticks.max(chapter.start_position_ticks),
        name: chapter.name,
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
    mapper.to_persisted(stream)
}

fn attachment_from_probe(attachment: ProbedMediaAttachment) -> PersistedMediaAttachment {
    PersistedMediaAttachment {
        attachment_index: attachment.index,
        codec: Some(attachment.codec),
        codec_tag: attachment.codec_tag,
        comment: attachment.comment,
        file_name: attachment.file_name,
        mime_type: attachment.mime_type,
        delivery_url: None,
    }
}

fn stream_from_probe(stream: ProbedMediaStream) -> PersistedMediaStream {
    let is_interlaced = stream.is_interlaced();
    let is_default = stream.is_default();
    let is_forced = stream.is_forced();
    let is_external = stream.is_external();
    let is_original = stream.is_original();
    let is_anamorphic = stream.is_anamorphic();
    let is_avc = stream.is_avc();
    let is_hearing_impaired = stream.is_hearing_impaired();
    PersistedMediaStream {
        stream_index: stream.index,
        stream_type: stream_type_from_probe(stream.stream_type),
        codec: non_empty_owned_string(stream.codec),
        language: stream.language,
        channel_layout: None,
        profile: stream.profile,
        aspect_ratio: stream.aspect_ratio,
        path: None,
        is_interlaced: Some(is_interlaced),
        bit_rate: stream.bit_rate.and_then(i32_from_i64),
        channels: stream.channels.and_then(i32_from_u32),
        sample_rate: None,
        is_default,
        is_forced,
        is_external,
        is_original,
        height: stream.height,
        width: stream.width,
        average_frame_rate: stream.average_frame_rate,
        real_frame_rate: stream.real_frame_rate,
        level: stream.level.map(f64_to_f32),
        pixel_format: stream.pixel_format,
        bit_depth: stream.bit_depth,
        is_anamorphic: Some(is_anamorphic),
        ref_frames: stream.ref_frames,
        codec_tag: None,
        comment: None,
        nal_length_size: stream.nal_length_size,
        is_avc: Some(is_avc),
        title: stream.title,
        time_base: stream.time_base,
        codec_time_base: stream.codec_time_base,
        color_range: stream.color_range,
        color_primaries: stream.color_primaries,
        color_space: stream.color_space,
        color_transfer: stream.color_transfer,
        dv_version_major: stream.dv_version_major,
        dv_version_minor: stream.dv_version_minor,
        dv_profile: stream.dv_profile,
        dv_level: stream.dv_level,
        rpu_present_flag: stream.rpu_present_flag,
        el_present_flag: stream.el_present_flag,
        bl_present_flag: stream.bl_present_flag,
        dv_bl_signal_compatibility_id: stream.dv_bl_signal_compatibility_id,
        is_hearing_impaired: Some(is_hearing_impaired),
        rotation: stream.rotation,
        hdr10_plus_present_flag: stream.hdr10_plus_present_flag,
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

fn non_empty_owned_string(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
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

fn set_additional_parts<I>(item: &mut base_item::Model, parts: &[I]) -> bool
where
    I: AsRef<str>,
{
    let mut data = item.data.take().unwrap_or_else(|| json!({}));
    let object = data
        .as_object_mut()
        .expect("base item metadata must be a JSON object");
    let previous = object
        .get("AdditionalParts")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if previous
        .iter()
        .map(String::as_str)
        .eq(parts.iter().map(AsRef::as_ref))
    {
        item.data = Some(data);
        return false;
    }
    object.insert(
        "AdditionalParts".to_owned(),
        Value::Array(
            parts
                .iter()
                .map(|part| Value::String(part.as_ref().to_owned()))
                .collect(),
        ),
    );
    item.data = Some(data);
    true
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
    "wmv", "strm",
];

const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "svg", "ico",
];

const BOOK_EXTENSIONS: &[&str] = &["pdf", "epub", "mobi", "cbr", "cbz", "cb7", "cbt", "djvu"];

#[cfg(test)]
mod tests {
    use super::{
        LibraryScanGuard, LibraryScanService, MediaKind, ScanLibraryKind, apply_non_movie_nfo,
        apply_probed_item_metadata, apply_strm_metadata, attachment_image_type,
        attachments_from_media_info, codec_from_extension, default_stream, display_name,
        extra_type_name, is_extras_directory, local_image_type, media_item_data, media_kind,
        merge_scan_summary, next_stream_index, read_strm_target, relations_from_movie_nfo,
        relations_from_nfo_metadata, resolve_external_subtitle_streams_from_entries,
        scan_nfo_person, set_additional_parts, stable_item_id, streams_from_media_info,
    };
    use chrono::Utc;
    use jellyfin_data::{PersistedMediaStreamType, entities::base_item};
    use jellyfin_media_encoding::probing::{ProbeContext, normalize_probe_json};
    use jellyfin_naming::ExtraType;
    use jellyfin_providers::media_info::MediaFileSystemEntry;
    use jellyfin_xbmc_metadata::{MovieNfo, NfoMetadata, NfoPerson, PersonKind};
    use serde_json::json;
    use std::{
        collections::HashSet,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    async fn measure_limiter_peak(
        service: Arc<LibraryScanService>,
        expected_limit: usize,
    ) -> usize {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let mut workers = Vec::new();

        // Model two library batches entering the shared limiter concurrently.
        for _batch in 0..2 {
            for _item in 0..4 {
                let service = Arc::clone(&service);
                let current = Arc::clone(&current);
                let peak = Arc::clone(&peak);
                let gate = Arc::clone(&gate);
                workers.push(tokio::spawn(async move {
                    let _permit = service.media_item_limiter.acquire().await;
                    let in_flight = current.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(in_flight, Ordering::AcqRel);
                    let gate_permit = gate.acquire().await.expect("test gate remains open");
                    current.fetch_sub(1, Ordering::AcqRel);
                    drop(gate_permit);
                }));
            }
        }

        tokio::time::timeout(Duration::from_secs(2), async {
            while current.load(Ordering::Acquire) < expected_limit {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("configured number of workers should enter the limiter");
        assert_eq!(current.load(Ordering::Acquire), expected_limit);

        gate.add_permits(workers.len());
        for worker in workers {
            worker.await.expect("limiter worker should finish");
        }
        assert_eq!(current.load(Ordering::Acquire), 0);
        peak.load(Ordering::Acquire)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn media_item_limiter_is_shared_across_batches_and_tracks_updates() {
        let service = Arc::new(LibraryScanService::new(
            sea_orm::DatabaseConnection::Disconnected,
        ));
        service.set_fanout_concurrency(2);
        assert_eq!(service.fanout_concurrency(), 2);
        assert_eq!(measure_limiter_peak(Arc::clone(&service), 2).await, 2);

        service.set_fanout_concurrency(3);
        assert_eq!(service.fanout_concurrency(), 3);
        assert_eq!(measure_limiter_peak(Arc::clone(&service), 3).await, 3);

        service.set_fanout_concurrency(0);
        assert_eq!(service.fanout_concurrency(), 1);
        assert_eq!(measure_limiter_peak(service, 1).await, 1);
    }

    #[test]
    fn dropping_scan_guard_releases_only_its_collections() {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let active_scans = Arc::new(Mutex::new(HashSet::from([first, second])));
        {
            let _guard = LibraryScanGuard {
                active_scans: Arc::clone(&active_scans),
                collection_ids: vec![first],
            };
            assert_eq!(active_scans.lock().unwrap().len(), 2);
        }
        assert_eq!(*active_scans.lock().unwrap(), HashSet::from([second]));
    }

    #[test]
    fn scan_reservations_allow_disjoint_libraries_and_reject_overlap() {
        let service = LibraryScanService::new(sea_orm::DatabaseConnection::Disconnected);
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();

        let first_guard = service.try_start_scans([first]).unwrap();
        let second_guard = service.try_start_scans([second]).unwrap();
        assert!(service.try_start_scans([first]).is_err());

        drop(first_guard);
        assert!(service.try_start_scans([first]).is_ok());
        drop(second_guard);
    }

    #[test]
    fn scan_summaries_merge_counts_and_changed_ids() {
        let added = uuid::Uuid::new_v4();
        let changed = uuid::Uuid::new_v4();
        let removed = uuid::Uuid::new_v4();
        let mut summary = super::LibraryScanSummary {
            folders_seen: 1,
            items_added: 1,
            items_seen: 2,
            added_ids: vec![added],
            ..Default::default()
        };

        merge_scan_summary(
            &mut summary,
            super::LibraryScanSummary {
                folders_seen: 1,
                items_removed: 1,
                items_seen: 3,
                changed_ids: vec![changed],
                removed_ids: vec![removed],
                ..Default::default()
            },
        );

        assert_eq!(summary.folders_seen, 2);
        assert_eq!(summary.items_added, 1);
        assert_eq!(summary.items_removed, 1);
        assert_eq!(summary.items_seen, 5);
        assert_eq!(summary.added_ids, [added]);
        assert_eq!(summary.changed_ids, [changed]);
        assert_eq!(summary.removed_ids, [removed]);
    }

    fn base_item_default() -> base_item::Model {
        base_item::Model {
            id: uuid::Uuid::nil(),
            item_type: "Movie".to_owned(),
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
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: Utc::now(),
            date_modified: Utc::now(),
            row_version: 1,
        }
    }

    #[test]
    fn media_kind_accepts_common_direct_play_extensions() {
        assert_eq!(media_kind(Path::new("movie.MKV")), Some(MediaKind::Video));
        assert_eq!(media_kind(Path::new("movie.StRm")), Some(MediaKind::Video));
        assert_eq!(media_kind(Path::new("song.FlAc")), Some(MediaKind::Audio));
        assert_eq!(media_kind(Path::new("photo.jpg")), Some(MediaKind::Photo));
        assert_eq!(media_kind(Path::new("book.pdf")), Some(MediaKind::Book));
        assert_eq!(media_kind(Path::new("data.nfo")), None);
    }

    #[tokio::test]
    async fn strm_target_uses_first_non_empty_trimmed_line() {
        let path =
            std::env::temp_dir().join(format!("jellyfin-strm-{}.strm", uuid::Uuid::new_v4()));
        tokio::fs::write(
            &path,
            "\n \u{feff} /CloudNAS/Movie/movie.mkv \r\nignored.mp4",
        )
        .await
        .unwrap();

        let target = read_strm_target(&path).await.unwrap();

        assert_eq!(target.as_deref(), Some("/CloudNAS/Movie/movie.mkv"));
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[test]
    fn strm_metadata_preserves_pointer_path_and_uses_target_container() {
        let mut item = base_item_default();
        item.path = Some("/library/Movie.strm".to_owned());

        assert!(apply_strm_metadata(
            &mut item,
            "/CloudNAS/Movie/movie.mkv",
            Some("/CloudNAS/Movie/movie.mkv")
        ));
        assert_eq!(item.path.as_deref(), Some("/library/Movie.strm"));
        assert_eq!(item.data.as_ref().unwrap()["Container"], "mkv");
        assert_eq!(
            item.data.as_ref().unwrap()["StrmTarget"],
            "/CloudNAS/Movie/movie.mkv"
        );
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
            "MusicVideo"
        );
        assert_eq!(
            ScanLibraryKind::from_collection_type(Some("music")).video_item_type(),
            "Video"
        );
        assert_eq!(
            ScanLibraryKind::from_collection_type(None).video_item_type(),
            "Video"
        );
        assert!(ScanLibraryKind::from_collection_type(Some("tvshows")).is_tv());
        assert!(!ScanLibraryKind::from_collection_type(Some("movies")).is_tv());
        assert!(ScanLibraryKind::from_collection_type(Some("music")).is_music());
        assert!(!ScanLibraryKind::from_collection_type(None).is_music());
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
        let mut media_info = normalize_probe_json(
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

        let streams = streams_from_media_info(&mut media_info);

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
        let mut media_info = normalize_probe_json(
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

        let streams = streams_from_media_info(&mut media_info);

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
        let mut media_info = normalize_probe_json(
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

        let attachments = attachments_from_media_info(&mut media_info);

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
    fn additional_parts_metadata_is_idempotent() {
        let mut item = base_item::Model {
            id: uuid::Uuid::new_v4(),
            item_type: "Movie".to_owned(),
            data: Some(json!({ "Name": "Movie" })),
            ..base_item_default()
        };
        let parts = vec!["/media/part2.mkv".to_owned(), "/media/part3.mkv".to_owned()];
        assert!(set_additional_parts(&mut item, &parts));
        assert_eq!(
            item.data
                .as_ref()
                .and_then(|data| data.get("AdditionalParts"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert!(!set_additional_parts(&mut item, &parts));
    }

    #[test]
    fn probed_item_metadata_updates_runtime_and_embedded_fields() {
        let mut media_info = normalize_probe_json(
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
            &mut media_info
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

    #[test]
    fn movie_nfo_relations_include_genres_studios_and_people() {
        let movie = MovieNfo {
            genres: vec!["Drama".to_owned(), "Crime".to_owned()],
            studios: vec!["Example Studio".to_owned()],
            people: vec![
                NfoPerson {
                    name: "Jane Actor".to_owned(),
                    role: "Lead".to_owned(),
                    kind: PersonKind::Actor,
                    sort_order: Some(0),
                    image_url: None,
                },
                NfoPerson {
                    name: "John Director".to_owned(),
                    role: "Director".to_owned(),
                    kind: PersonKind::Director,
                    sort_order: Some(1),
                    image_url: None,
                },
            ],
            ..MovieNfo::default()
        };

        let relations = relations_from_movie_nfo(movie);
        assert_eq!(relations.genres, ["Drama", "Crime"]);
        assert!(relations.tags.is_empty());
        assert_eq!(relations.studios, ["Example Studio"]);
        assert_eq!(relations.people.len(), 2);
        assert_eq!(relations.people[0].name, "Jane Actor");
        assert_eq!(relations.people[0].person_type, "Actor");
        assert_eq!(relations.people[0].role, "Lead");
        assert_eq!(relations.people[1].person_type, "Director");
    }

    #[test]
    fn generic_nfo_relations_include_tags_and_normalized_person_types() {
        let metadata = NfoMetadata {
            genres: vec!["Sci-Fi".to_owned()],
            tags: vec!["anthology".to_owned()],
            studios: vec!["Network".to_owned()],
            people: vec![NfoPerson {
                name: "Jane Writer".to_owned(),
                role: "Writer".to_owned(),
                kind: PersonKind::Writer,
                sort_order: Some(2),
                image_url: None,
            }],
            ..NfoMetadata::default()
        };

        let relations = relations_from_nfo_metadata(metadata);
        assert_eq!(relations.genres, ["Sci-Fi"]);
        assert_eq!(relations.tags, ["anthology"]);
        assert_eq!(relations.studios, ["Network"]);
        assert_eq!(relations.people[0].person_type, "Writer");
        assert_eq!(relations.people[0].sort_order, Some(2));
    }

    #[test]
    fn unknown_nfo_person_kinds_fall_back_to_actor() {
        let person = scan_nfo_person(NfoPerson {
            name: "Uncredited".to_owned(),
            role: String::new(),
            kind: PersonKind::Other(String::new()),
            sort_order: None,
            image_url: None,
        });
        assert_eq!(person.person_type, "Actor");
    }
}
