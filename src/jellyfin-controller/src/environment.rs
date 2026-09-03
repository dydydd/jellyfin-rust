use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use jellyfin_model::{FileSystemEntryInfo, FileSystemEntryType};
use sysinfo::{DiskRefreshKind, Disks};
use thiserror::Error;
use uuid::Uuid;

const UNC_SEPARATOR: char = '\\';
const UNC_START_PREFIX: &str = "\\\\";
const PROBE_ATTEMPTS: usize = 16;

#[derive(Debug, Error)]
pub enum EnvironmentError {
    #[error("path was not found")]
    NotFound,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Uncached access to the server file system used by the environment API.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvironmentService;

impl EnvironmentService {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Enumerates current direct children, applying the environment API filters.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error instead of treating a missing
    /// directory as an empty directory.
    pub fn directory_contents(
        &self,
        path: &str,
        include_files: bool,
        include_directories: bool,
    ) -> Result<Vec<FileSystemEntryInfo>, EnvironmentError> {
        if is_bare_unc_host(path) {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let is_directory = metadata.is_dir();
            if (is_directory && !include_directories) || (!is_directory && !include_files) {
                continue;
            }
            let full_path = path_to_string(&entry.path());
            let entry_type = if is_directory {
                FileSystemEntryType::Directory
            } else {
                FileSystemEntryType::File
            };
            entries.push(FileSystemEntryInfo::new(
                entry.file_name().to_string_lossy(),
                full_path,
                entry_type,
            ));
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    /// Validates existence, expected type, and optionally directory writability.
    ///
    /// `validate_writable` intentionally applies only when `is_file` is absent,
    /// matching Jellyfin's controller branch ordering.
    ///
    /// # Errors
    ///
    /// Returns [`EnvironmentError::NotFound`] for a missing or mismatched path,
    /// and an I/O error when the writable probe cannot be created or removed.
    pub fn validate_path(
        &self,
        path: Option<&str>,
        is_file: Option<bool>,
        validate_writable: bool,
    ) -> Result<(), EnvironmentError> {
        let path = path.map(Path::new);
        match is_file {
            Some(true) if path.is_none_or(|path| !path.is_file()) => {
                return Err(EnvironmentError::NotFound);
            }
            Some(false) if path.is_none_or(|path| !path.is_dir()) => {
                return Err(EnvironmentError::NotFound);
            }
            Some(_) => return Ok(()),
            None if path.is_none_or(|path| !path.is_file() && !path.is_dir()) => {
                return Err(EnvironmentError::NotFound);
            }
            None => {}
        }

        if validate_writable {
            let Some(path) = path else {
                return Err(EnvironmentError::NotFound);
            };
            writable_probe(path)?;
        }
        Ok(())
    }

    /// Lists ready, nonempty fixed, removable, and network-backed mounts.
    #[must_use]
    pub fn drives(&self) -> Vec<FileSystemEntryInfo> {
        let disks =
            Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
        let mut seen = HashSet::<PathBuf>::new();
        disks
            .list()
            .iter()
            .filter(|disk| disk.total_space() != 0)
            .filter_map(|disk| {
                let mount = disk.mount_point();
                if !mount.is_dir() || !seen.insert(mount.to_path_buf()) {
                    return None;
                }
                let path = path_to_string(mount);
                Some(FileSystemEntryInfo::directory(path.clone(), path))
            })
            .collect()
    }

    /// Returns the platform-native parent with Jellyfin's UNC fallback.
    #[must_use]
    pub fn parent_path(&self, path: &str) -> Option<String> {
        if let Some(parent) = Path::new(path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            return Some(path_to_string(parent));
        }

        let index = path.rfind(UNC_SEPARATOR)?;
        if !path.starts_with(UNC_SEPARATOR) {
            return None;
        }
        let parent = &path[..index];
        if parent.trim_start_matches(UNC_SEPARATOR).trim().is_empty() {
            None
        } else {
            Some(parent.to_owned())
        }
    }
}

fn is_bare_unc_host(path: &str) -> bool {
    path.starts_with(UNC_START_PREFIX) && path.rfind(UNC_SEPARATOR) == Some(1)
}

fn writable_probe(directory: &Path) -> io::Result<()> {
    for _ in 0..PROBE_ATTEMPTS {
        let path = directory.join(Uuid::new_v4().to_string());
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                drop(file);
                return ProbeFile::new(path).cleanup();
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate a unique writable probe",
    ))
}

struct ProbeFile {
    path: Option<PathBuf>,
}

impl ProbeFile {
    const fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn cleanup(mut self) -> io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        fs::remove_file(path)?;
        self.path = None;
        Ok(())
    }
}

impl Drop for ProbeFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
