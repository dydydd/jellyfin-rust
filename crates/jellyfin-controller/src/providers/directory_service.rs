use std::{
    collections::HashMap,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError, RwLock},
    time::SystemTime,
};

/// Metadata for a file-system entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileSystemMetadata {
    pub exists: bool,
    pub full_name: PathBuf,
    pub name: OsString,
    pub extension: Option<OsString>,
    pub length: u64,
    pub last_write_time: Option<SystemTime>,
    pub creation_time: Option<SystemTime>,
    pub is_directory: bool,
}

/// File-system operations used by [`DirectoryService`].
pub trait FileSystem: Send + Sync {
    /// Enumerates direct child files and directories.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error when enumeration fails.
    fn get_file_system_entries(&self, path: &Path) -> io::Result<Vec<FileSystemMetadata>>;

    /// Reads metadata for one path, returning `exists = false` when absent.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error when metadata cannot be read.
    fn get_file_system_info(&self, path: &Path) -> io::Result<FileSystemMetadata>;

    /// Enumerates direct child file paths.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error when enumeration fails.
    fn get_file_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// Enumerates direct child file-system entry paths.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error when enumeration fails.
    fn get_file_system_entry_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
}

/// Standard-library-backed local file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFileSystem;

impl FileSystem for LocalFileSystem {
    fn get_file_system_entries(&self, path: &Path) -> io::Result<Vec<FileSystemMetadata>> {
        fs::read_dir(path)?
            .map(|entry| entry.and_then(|entry| metadata_for_path(&entry.path())))
            .collect()
    }

    fn get_file_system_info(&self, path: &Path) -> io::Result<FileSystemMetadata> {
        match metadata_for_path(path) {
            Ok(metadata) => Ok(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(FileSystemMetadata {
                full_name: path.to_path_buf(),
                ..FileSystemMetadata::default()
            }),
            Err(error) => Err(error),
        }
    }

    fn get_file_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.metadata()?.is_file() {
                paths.push(entry.path());
            }
        }
        Ok(paths)
    }

    fn get_file_system_entry_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }
}

/// Instance-scoped directory and file metadata cache.
pub struct DirectoryService<F> {
    file_system: F,
    entry_cache: RwLock<HashMap<PathBuf, Arc<[FileSystemMetadata]>>>,
    file_cache: RwLock<HashMap<PathBuf, FileSystemMetadata>>,
    file_path_cache: RwLock<HashMap<PathBuf, Arc<[PathBuf]>>>,
}

impl Default for DirectoryService<LocalFileSystem> {
    fn default() -> Self {
        Self::new(LocalFileSystem)
    }
}

impl<F: FileSystem> DirectoryService<F> {
    #[must_use]
    pub fn new(file_system: F) -> Self {
        Self {
            file_system,
            entry_cache: RwLock::new(HashMap::new()),
            file_cache: RwLock::new(HashMap::new()),
            file_path_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Returns a cached snapshot of direct file-system entries.
    ///
    /// # Errors
    ///
    /// Returns an I/O error other than a missing directory, or a cache-lock
    /// error after another thread panics while holding the lock.
    pub fn get_file_system_entries(
        &self,
        path: impl AsRef<Path>,
    ) -> io::Result<Arc<[FileSystemMetadata]>> {
        let path = path.as_ref();
        if let Some(entries) = self
            .entry_cache
            .read()
            .map_err(cache_error)?
            .get(path)
            .cloned()
        {
            return Ok(entries);
        }

        let entries = match self.file_system.get_file_system_entries(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let entries = Arc::<[FileSystemMetadata]>::from(entries);
        let mut cache = self.entry_cache.write().map_err(cache_error)?;
        Ok(cache
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::clone(&entries))
            .clone())
    }

    /// Returns cached direct child directories.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::get_file_system_entries`].
    pub fn get_directories(&self, path: impl AsRef<Path>) -> io::Result<Vec<FileSystemMetadata>> {
        Ok(self
            .get_file_system_entries(path)?
            .iter()
            .filter(|entry| entry.is_directory)
            .cloned()
            .collect())
    }

    /// Returns cached direct child files.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::get_file_system_entries`].
    pub fn get_files(&self, path: impl AsRef<Path>) -> io::Result<Vec<FileSystemMetadata>> {
        Ok(self
            .get_file_system_entries(path)?
            .iter()
            .filter(|entry| !entry.is_directory)
            .cloned()
            .collect())
    }

    /// Returns cached metadata only when `path` is an existing file.
    ///
    /// # Errors
    ///
    /// Returns an underlying file-system or cache-lock error.
    pub fn get_file(&self, path: impl AsRef<Path>) -> io::Result<Option<FileSystemMetadata>> {
        Ok(self
            .get_file_system_entry(path)?
            .filter(|entry| !entry.is_directory))
    }

    /// Returns cached metadata only when `path` is an existing directory.
    ///
    /// # Errors
    ///
    /// Returns an underlying file-system or cache-lock error.
    pub fn get_directory(&self, path: impl AsRef<Path>) -> io::Result<Option<FileSystemMetadata>> {
        Ok(self
            .get_file_system_entry(path)?
            .filter(|entry| entry.is_directory))
    }

    /// Returns metadata for one existing path and caches successful lookups.
    ///
    /// # Errors
    ///
    /// Returns an underlying file-system or cache-lock error.
    pub fn get_file_system_entry(
        &self,
        path: impl AsRef<Path>,
    ) -> io::Result<Option<FileSystemMetadata>> {
        let path = path.as_ref();
        if let Some(entry) = self
            .file_cache
            .read()
            .map_err(cache_error)?
            .get(path)
            .cloned()
        {
            return Ok(Some(entry));
        }

        let entry = self.file_system.get_file_system_info(path)?;
        if !entry.exists {
            return Ok(None);
        }

        let mut cache = self.file_cache.write().map_err(cache_error)?;
        Ok(Some(
            cache.entry(path.to_path_buf()).or_insert(entry).clone(),
        ))
    }

    /// Returns sorted direct child file paths from the instance cache.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::get_file_paths_with_options`].
    pub fn get_file_paths(&self, path: impl AsRef<Path>) -> io::Result<Arc<[PathBuf]>> {
        self.get_file_paths_with_options(path, false)
    }

    /// Returns sorted direct child file paths, optionally clearing one key.
    ///
    /// # Errors
    ///
    /// Returns an I/O error other than a missing directory, or a cache-lock
    /// error after another thread panics while holding the lock.
    pub fn get_file_paths_with_options(
        &self,
        path: impl AsRef<Path>,
        clear_cache: bool,
    ) -> io::Result<Arc<[PathBuf]>> {
        let path = path.as_ref();
        if clear_cache {
            self.file_path_cache
                .write()
                .map_err(cache_error)?
                .remove(path);
        } else if let Some(paths) = self
            .file_path_cache
            .read()
            .map_err(cache_error)?
            .get(path)
            .cloned()
        {
            return Ok(paths);
        }

        let mut paths = match self.file_system.get_file_paths(path) {
            Ok(paths) => paths,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        paths.sort_unstable();
        let paths = Arc::<[PathBuf]>::from(paths);
        let mut cache = self.file_path_cache.write().map_err(cache_error)?;
        Ok(cache
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::clone(&paths))
            .clone())
    }

    /// Returns whether `path` has at least one accessible direct child.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error when enumeration fails.
    pub fn is_accessible(&self, path: impl AsRef<Path>) -> io::Result<bool> {
        Ok(!self
            .file_system
            .get_file_system_entry_paths(path.as_ref())?
            .is_empty())
    }
}

fn metadata_for_path(path: &Path) -> io::Result<FileSystemMetadata> {
    let metadata = fs::metadata(path)?;
    Ok(FileSystemMetadata {
        exists: true,
        full_name: path.to_path_buf(),
        name: path.file_name().unwrap_or_default().to_os_string(),
        extension: path.extension().map(|extension| {
            let mut value = OsString::from(".");
            value.push(extension);
            value
        }),
        length: metadata.len(),
        last_write_time: metadata.modified().ok(),
        creation_time: metadata.created().ok(),
        is_directory: metadata.is_dir(),
    })
}

fn cache_error<T>(_error: PoisonError<T>) -> io::Error {
    io::Error::other("directory service cache lock is poisoned")
}
