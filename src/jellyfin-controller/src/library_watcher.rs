use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing;

use crate::{LibraryScanService, VirtualFolder, VirtualFolderService};

const DEBOUNCE_SECS: u64 = 5;
const MAX_QUEUED_EVENTS: usize = 1024;
const MAX_PENDING_PATHS: usize = 1024;

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

        let (tx, rx) =
            std::sync::mpsc::sync_channel::<Result<Event, notify::Error>>(MAX_QUEUED_EVENTS);
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                if let Err(std::sync::mpsc::TrySendError::Full(_)) = tx.try_send(event) {
                    tracing::debug!(
                        capacity = MAX_QUEUED_EVENTS,
                        "file watcher event queue is full; coalescing burst"
                    );
                }
            },
            Config::default(),
        )
        .map_err(LibraryWatcherError::Notify)?;

        if register_library_paths(self.paths, |path| {
            watcher.watch(path, RecursiveMode::Recursive)
        }) == 0
        {
            return Ok(());
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
                let mut pending = HashSet::<PathBuf>::new();
                let mut last_event = std::time::Instant::now();

                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(Ok(event)) => {
                            for path in relevant_event_paths(event) {
                                pending.insert(path);
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
                        && (last_event.elapsed() >= Duration::from_secs(DEBOUNCE_SECS)
                            || pending.len() >= MAX_PENDING_PATHS)
                    {
                        let dirs = deduplicate_parents(pending.iter());
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

                            let mut canonical_dirs = Vec::with_capacity(dirs.len());
                            for path in &dirs {
                                let Ok(canonical) = tokio::fs::canonicalize(path).await else {
                                    continue;
                                };
                                canonical_dirs.push(canonical);
                            }
                            let affected = affected_folder_ids(&canonical_dirs, &all_virtual);
                            for vf in all_virtual
                                .iter()
                                .filter(|folder| affected.contains(&folder.id))
                            {
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

fn register_library_paths(
    mut paths: Vec<PathBuf>,
    mut watch: impl FnMut(&Path) -> notify::Result<()>,
) -> usize {
    // LibraryMonitor.Start orders roots and suppresses descendants of an
    // existing recursive watch. Retain only successful roots so one failure
    // does not suppress another configured directory that can be watched.
    paths.sort_unstable();
    paths.dedup();
    let mut watched = Vec::<PathBuf>::new();
    for path in paths {
        if watched.iter().any(|parent| path.starts_with(parent)) {
            continue;
        }
        // LibraryMonitor.StartWatchingPath skips unavailable directories and
        // catches registration errors independently for each configured path.
        if !path.is_dir() {
            tracing::info!(path = %path.display(), "skipping realtime monitor for unavailable directory");
            continue;
        }
        match watch(&path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "watching library directory");
                watched.push(path);
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot watch library directory");
            }
        }
    }
    watched.len()
}

fn relevant_event_paths(event: Event) -> impl Iterator<Item = PathBuf> {
    let relevant = matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
    );
    event
        .paths
        .into_iter()
        .filter(move |path| relevant && is_library_media_path(path))
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

fn deduplicate_parents<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
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

fn affected_folder_ids(paths: &[PathBuf], folders: &[VirtualFolder]) -> HashSet<uuid::Uuid> {
    folders
        .iter()
        .filter(|folder| {
            paths.iter().any(|path| {
                folder
                    .locations
                    .iter()
                    .any(|location| path.starts_with(Path::new(location)))
            })
        })
        .map(|folder| folder.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_registration_isolates_unavailable_paths_and_registration_errors() {
        let root = std::env::temp_dir().join(format!(
            "jellyfin-watcher-registration-{}",
            uuid::Uuid::new_v4()
        ));
        let first = root.join("first");
        let failure = root.join("registration-failure");
        let last = root.join("z-last");
        for path in [&first, &failure, &last] {
            std::fs::create_dir_all(path).unwrap();
        }
        let regular_file = root.join("file.mkv");
        std::fs::write(&regular_file, []).unwrap();
        let mut attempts = Vec::new();
        let registered = register_library_paths(
            vec![
                last.clone(),
                root.join("missing"),
                failure.clone(),
                regular_file,
                first.clone(),
            ],
            |path| {
                attempts.push(path.to_path_buf());
                if path == failure {
                    Err(notify::Error::generic("simulated registration failure"))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(registered, 2);
        assert_eq!(attempts, [first, failure, last]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recursive_watch_registration_deduplicates_parent_and_child_roots() {
        let root =
            std::env::temp_dir().join(format!("jellyfin-watcher-roots-{}", uuid::Uuid::new_v4()));
        let parent = root.join("movies");
        let child = parent.join("collection");
        let sibling = root.join("movies-other");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let mut attempts = Vec::new();
        let registered = register_library_paths(
            vec![child, sibling.clone(), parent.clone(), parent.clone()],
            |path| {
                attempts.push(path.to_path_buf());
                Ok(())
            },
        );
        assert_eq!(registered, 2);
        assert_eq!(attempts, [parent, sibling]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_parent_registration_does_not_hide_a_watchable_child() {
        let root =
            std::env::temp_dir().join(format!("jellyfin-watcher-parent-{}", uuid::Uuid::new_v4()));
        let child = root.join("movies");
        std::fs::create_dir_all(&child).unwrap();
        let mut attempts = Vec::new();
        let registered = register_library_paths(vec![child.clone(), root.clone()], |path| {
            attempts.push(path.to_path_buf());
            if path == root {
                Err(notify::Error::generic(
                    "simulated parent registration failure",
                ))
            } else {
                Ok(())
            }
        });
        assert_eq!(registered, 1);
        assert_eq!(attempts, [root.clone(), child]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_roots_leave_no_registered_watch() {
        let missing =
            std::env::temp_dir().join(format!("jellyfin-watcher-missing-{}", uuid::Uuid::new_v4()));
        assert_eq!(
            register_library_paths(vec![missing], |_| panic!(
                "missing directory must be skipped"
            )),
            0
        );
    }

    #[test]
    fn event_paths_are_bounded_to_media_and_all_paths_are_retained() {
        let event = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![
                PathBuf::from("/media/movies/one.mkv"),
                PathBuf::from("/media/movies/two.srt"),
                PathBuf::from("/media/movies/readme.txt"),
            ],
            attrs: notify::event::EventAttributes::new(),
        };
        assert_eq!(relevant_event_paths(event).count(), 2);
    }

    #[test]
    fn changed_directories_coalesce_to_one_scan_per_virtual_folder() {
        let movie_id = uuid::Uuid::new_v4();
        let other_id = uuid::Uuid::new_v4();
        let folders = vec![
            virtual_folder(movie_id, &["/media/movies"]),
            virtual_folder(other_id, &["/media/other"]),
        ];
        let affected = affected_folder_ids(
            &[
                PathBuf::from("/media/movies/a"),
                PathBuf::from("/media/movies/b"),
                PathBuf::from("/media/movies/a/extras"),
            ],
            &folders,
        );
        assert_eq!(affected, HashSet::from([movie_id]));
    }

    fn virtual_folder(id: uuid::Uuid, locations: &[&str]) -> VirtualFolder {
        VirtualFolder {
            id,
            name: id.to_string(),
            collection_type: Some("movies".to_owned()),
            library_options: serde_json::Value::Null,
            locations: locations.iter().map(|path| (*path).to_owned()).collect(),
            refresh_requested: false,
        }
    }
}
