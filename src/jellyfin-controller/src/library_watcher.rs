use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing;

use crate::{LibraryScanService, VirtualFolderService};

const DEBOUNCE_SECS: u64 = 5;

pub struct LibraryWatcher {
    scan: Arc<LibraryScanService>,
    folders: Arc<VirtualFolderService>,
    paths: Vec<PathBuf>,
}

impl LibraryWatcher {
    #[must_use]
    pub fn new(
        scan: Arc<LibraryScanService>,
        folders: Arc<VirtualFolderService>,
        paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            scan,
            folders,
            paths,
        }
    }

    /// Starts watching library directories. Runs the watcher in a
    /// background thread; file changes trigger a scan of only the
    /// affected virtual folder after a 5-second debounce window.
    ///
    /// # Errors
    ///
    /// Returns an error when the filesystem watcher cannot be created.
    pub fn start(self) -> Result<(), LibraryWatcherError> {
        if self.paths.is_empty() {
            return Ok(());
        }

        let (tx, rx) = std::sync::mpsc::channel::<Result<Event, notify::Error>>();
        let mut watcher =
            RecommendedWatcher::new(tx, Config::default()).map_err(LibraryWatcherError::Notify)?;

        for path in self.paths {
            if let Err(error) = watcher.watch(&path, RecursiveMode::Recursive) {
                return Err(LibraryWatcherError::Watch(path, error));
            }
            tracing::info!(path = %path.display(), "watching library directory");
        }

        let scan = self.scan;
        let folders = self.folders;
        let runtime = tokio::runtime::Handle::current();

        // Spawn a thread for the blocking event loop
        std::thread::Builder::new()
            .name("jellyfin-library-watcher".into())
            .spawn(move || {
                // Keep the OS watcher alive for as long as the event loop.
                // Dropping it here would disconnect `rx` immediately.
                let _watcher = watcher;
                let mut pending: Vec<PathBuf> = Vec::new();
                let mut last_event = std::time::Instant::now();

                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(Ok(event)) => {
                            if let Some(path) = relevant_event_path(event) {
                                pending.push(path);
                                last_event = std::time::Instant::now();
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::debug!(error = %e, "file watcher error");
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            tracing::warn!("file watcher channel closed");
                            break;
                        }
                    }

                    if !pending.is_empty()
                        && last_event.elapsed() >= Duration::from_secs(DEBOUNCE_SECS)
                    {
                        let dirs = deduplicate_parents(&pending);
                        pending.clear();

                        tracing::debug!(
                            directories = dirs.len(),
                            "file change detected, triggering incremental scan"
                        );
                        runtime.block_on(async {
                            // Find which virtual folders contain the changed paths
                            let all_virtual = match folders.list().await {
                                Ok(v) => v,
                                Err(error) => {
                                    tracing::error!(%error,
                                            "cannot list virtual folders for incremental scan");
                                    return;
                                }
                            };

                            for path in &dirs {
                                let Ok(canonical) = tokio::fs::canonicalize(path).await else {
                                    continue;
                                };
                                let canonical_str = canonical.to_string_lossy();

                                for vf in &all_virtual {
                                    // Check if the changed path is under this virtual
                                    // folder's configured media paths
                                    let matches = vf
                                        .locations
                                        .iter()
                                        .any(|loc| canonical_str.starts_with(loc));
                                    if matches {
                                        if let Err(error) = scan.scan_collection(vf.id).await {
                                            tracing::error!(
                                                %error, folder = %vf.name,
                                                "incremental scan failed",
                                            );
                                        } else {
                                            tracing::debug!(
                                                folder = %vf.name,
                                                "incremental scan completed",
                                            );
                                        }
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
            })
            .map_err(LibraryWatcherError::Thread)?;

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryWatcherError {
    #[error("file system watcher error: {0}")]
    Notify(#[from] notify::Error),
    #[error("cannot watch {0}: {1}")]
    Watch(PathBuf, notify::Error),
    #[error("cannot spawn watcher thread: {0}")]
    Thread(std::io::Error),
}

fn relevant_event_path(event: Event) -> Option<PathBuf> {
    match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_) => {}
        _ => return None,
    }
    event
        .paths
        .into_iter()
        .next()
        .filter(|path| is_library_media_path(path))
}

fn is_library_media_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mkv"
            | "mp4"
            | "avi"
            | "mov"
            | "m4v"
            | "wmv"
            | "flv"
            | "webm"
            | "mp3"
            | "flac"
            | "aac"
            | "ogg"
            | "wav"
            | "m4a"
            | "opus"
            | "wma"
            | "dsf"
            | "aiff"
            | "srt"
            | "ass"
            | "ssa"
            | "sub"
            | "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "bmp"
            | "webp"
            | "tiff"
            | "tif"
            | "pdf"
            | "epub"
            | "mobi"
            | "cbr"
            | "cbz"
            | "djvu"
    )
}

fn deduplicate_parents(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();
    for path in paths {
        if let Some(parent) = path.parent()
            && !result.iter().any(|p| path.starts_with(p))
        {
            result.retain(|p| !p.starts_with(parent));
            result.push(parent.to_path_buf());
        }
    }
    result
}
