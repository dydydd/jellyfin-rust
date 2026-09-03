use std::{fs::OpenOptions, io, path::Path};

/// Creates a new empty file, or truncates an existing file.
///
/// The file is opened for both reading and writing, matching Jellyfin's
/// `FileHelper.CreateEmpty` contract, and is closed before this function
/// returns.
///
/// # Errors
///
/// Returns the underlying filesystem error when the file cannot be opened.
pub fn create_empty(path: impl AsRef<Path>) -> io::Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map(drop)
}

/// Namespace-compatible access to filesystem helper functions.
pub struct FileHelper;

impl FileHelper {
    /// Creates a new empty file, or truncates an existing file.
    ///
    /// # Errors
    ///
    /// Returns the underlying filesystem error when the file cannot be opened.
    pub fn create_empty(path: impl AsRef<Path>) -> io::Result<()> {
        create_empty(path)
    }
}
