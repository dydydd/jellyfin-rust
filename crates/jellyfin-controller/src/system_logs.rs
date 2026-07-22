use std::{
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::fs::File;

/// Metadata exposed by Jellyfin's server-log listing endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemLogFile {
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
    pub size: i64,
    pub name: String,
}

/// An opened server log whose path has already passed containment checks.
pub struct OpenedSystemLog {
    file: File,
}

impl OpenedSystemLog {
    /// Consumes the validated log and returns its open file handle.
    #[must_use]
    pub fn into_file(self) -> File {
        self.file
    }
}

#[derive(Debug, Error)]
pub enum SystemLogError {
    #[error("log file was not found")]
    NotFound,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Resolves readable files from the configured top-level server log folder.
#[derive(Debug, Clone)]
pub struct SystemLogService {
    log_directory: Arc<PathBuf>,
}

impl Default for SystemLogService {
    fn default() -> Self {
        Self::new("logs")
    }
}

impl SystemLogService {
    #[must_use]
    pub fn new(log_directory: impl Into<PathBuf>) -> Self {
        Self {
            log_directory: Arc::new(log_directory.into()),
        }
    }

    /// Lists top-level `.txt` and `.log` files using Jellyfin's stable order.
    ///
    /// Filesystem enumeration failures produce an empty list, matching the
    /// official controller's best-effort behavior for unavailable log paths.
    pub async fn list(&self) -> Vec<SystemLogFile> {
        let Ok(mut entries) = tokio::fs::read_dir(self.log_directory.as_ref()).await else {
            return Vec::new();
        };
        let mut files = Vec::new();
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => return Vec::new(),
            };
            let Ok(file_type) = entry.file_type().await else {
                return Vec::new();
            };
            if !file_type.is_file() || !is_supported_log_path(&entry.path()) {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                return Vec::new();
            };
            let Ok(size) = i64::try_from(metadata.len()) else {
                return Vec::new();
            };
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let created = metadata.created().unwrap_or(modified);
            files.push(SystemLogFile {
                date_created: created.into(),
                date_modified: modified.into(),
                size,
                name: entry.file_name().to_string_lossy().into_owned(),
            });
        }

        files.sort_by(|left, right| {
            right
                .date_modified
                .cmp(&left.date_modified)
                .then_with(|| right.date_created.cmp(&left.date_created))
                .then_with(|| left.name.cmp(&right.name))
        });
        files
    }

    /// Opens the unique top-level log whose basename equals `name`
    /// case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns [`SystemLogError::NotFound`] for missing, nested, ambiguous,
    /// non-file, or symlinked entries. Other filesystem failures are returned
    /// as [`SystemLogError::Io`].
    pub async fn open(&self, name: &str) -> Result<OpenedSystemLog, SystemLogError> {
        if !is_top_level_basename(name) {
            return Err(SystemLogError::NotFound);
        }

        let root = tokio::fs::canonicalize(self.log_directory.as_ref())
            .await
            .map_err(classify_io_error)?;
        if !tokio::fs::metadata(&root)
            .await
            .map_err(classify_io_error)?
            .is_dir()
        {
            return Err(SystemLogError::NotFound);
        }

        let mut entries = tokio::fs::read_dir(&root)
            .await
            .map_err(classify_io_error)?;
        let mut candidate = None;
        while let Some(entry) = entries.next_entry().await.map_err(classify_io_error)? {
            let matches = entry
                .file_name()
                .to_str()
                .is_some_and(|entry_name| unicode_ordinal_ignore_case(entry_name, name));
            if !matches {
                continue;
            }
            if candidate.replace(entry.path()).is_some() {
                return Err(SystemLogError::NotFound);
            }
        }
        let candidate = candidate.ok_or(SystemLogError::NotFound)?;
        let metadata = tokio::fs::symlink_metadata(&candidate)
            .await
            .map_err(classify_io_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SystemLogError::NotFound);
        }

        let candidate = tokio::fs::canonicalize(candidate)
            .await
            .map_err(classify_io_error)?;
        if !candidate.starts_with(&root) {
            return Err(SystemLogError::NotFound);
        }
        let file = File::open(candidate).await.map_err(classify_io_error)?;
        if !file.metadata().await.map_err(classify_io_error)?.is_file() {
            return Err(SystemLogError::NotFound);
        }

        Ok(OpenedSystemLog { file })
    }
}

fn is_supported_log_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("txt") || extension.eq_ignore_ascii_case("log")
        })
}

fn unicode_ordinal_ignore_case(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_uppercase)
        .eq(right.chars().flat_map(char::to_uppercase))
}

fn is_top_level_basename(name: &str) -> bool {
    if name.trim().is_empty() || name.contains('/') || name.contains('\\') {
        return false;
    }

    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn classify_io_error(error: io::Error) -> SystemLogError {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    ) {
        SystemLogError::NotFound
    } else {
        SystemLogError::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::io::AsyncReadExt;
    use uuid::Uuid;

    use super::{SystemLogError, SystemLogService};

    #[tokio::test]
    async fn unicode_case_variant_opens_the_unique_top_level_log() {
        let root = temporary_log_directory();
        tokio::fs::write(root.join("Épisode.LOG"), b"unicode log")
            .await
            .unwrap();

        let mut file = SystemLogService::new(&root)
            .open("éPISODE.log")
            .await
            .unwrap()
            .into_file();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).await.unwrap();

        assert_eq!(contents, b"unicode log");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn unicode_case_variant_ambiguity_is_not_found() {
        let root = temporary_log_directory();
        tokio::fs::write(root.join("Écho.log"), b"first")
            .await
            .unwrap();
        tokio::fs::write(root.join("éCHO.LOG"), b"second")
            .await
            .unwrap();

        let result = SystemLogService::new(&root).open("écho.log").await;

        assert!(matches!(result, Err(SystemLogError::NotFound)));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    fn temporary_log_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-system-log-controller-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
