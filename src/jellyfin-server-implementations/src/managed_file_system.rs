use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;
use uuid::Uuid;

/// Metadata returned by [`ManagedFileSystem::get_file_info`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedFileInfo {
    pub exists: bool,
    pub length: Option<u64>,
}

/// A typed file-system operation failure.
#[derive(Debug, Error)]
#[error("failed to {operation} at {path}: {source}")]
pub struct ManagedFileSystemError {
    pub operation: &'static str,
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}

impl ManagedFileSystemError {
    /// Returns the underlying I/O error kind.
    #[must_use]
    pub fn kind(&self) -> io::ErrorKind {
        self.source.kind()
    }
}

/// Jellyfin's managed file-system path and move operations.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManagedFileSystem;

impl ManagedFileSystem {
    /// Resolves a relative file path against its containing folder.
    ///
    /// Windows and Unix syntax are parsed lexically so callers can process
    /// either style independently of the server host operating system.
    #[must_use]
    pub fn make_absolute_path(folder_path: &str, file_path: &str) -> String {
        if file_path.trim().is_empty() || is_absolute_path(file_path) {
            return file_path.to_owned();
        }

        let file_path = file_path.strip_prefix('\\').unwrap_or(file_path);
        if uses_windows_syntax(folder_path) {
            normalize_windows_path(folder_path, file_path)
        } else {
            normalize_unix_path(folder_path, file_path)
        }
    }

    /// Replaces every character Jellyfin disallows in a filename with a space.
    #[must_use]
    pub fn get_valid_filename(filename: &str) -> String {
        filename
            .chars()
            .map(|character| {
                if is_invalid_filename_character(character) {
                    ' '
                } else {
                    character
                }
            })
            .collect()
    }

    /// Returns file metadata while treating missing files, directories, and
    /// dangling symbolic links as non-existent files.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O error when metadata cannot be read for a reason
    /// other than the path or symbolic-link target not existing.
    pub fn get_file_info(
        path: impl AsRef<Path>,
    ) -> Result<ManagedFileInfo, ManagedFileSystemError> {
        let path = path.as_ref();
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(ManagedFileInfo {
                exists: true,
                length: Some(metadata.len()),
            }),
            Ok(_) => Ok(ManagedFileInfo {
                exists: false,
                length: None,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ManagedFileInfo {
                exists: false,
                length: None,
            }),
            Err(source) => Err(operation_error("read file metadata", path, source)),
        }
    }

    /// Moves a directory, falling back to a staged recursive copy only when
    /// the source and destination are on different file systems.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O error if the destination exists, the initial rename
    /// fails for a reason other than a cross-device boundary, copying is not
    /// completed, publishing the staged copy fails, or the copied source cannot
    /// be removed. Copy failures leave the source untouched.
    pub fn move_directory(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<(), ManagedFileSystemError> {
        let source = source.as_ref();
        let destination = destination.as_ref();
        ensure_destination_absent(destination)?;

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| operation_error("create destination parent", parent, source))?;
        }

        match fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
            Err(source_error) => {
                return Err(operation_error("rename directory", source, source_error));
            }
        }

        let destination_parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let mut staging = StagingDirectory::create(destination_parent)?;
        copy_directory_contents(source, staging.path())
            .map_err(|source_error| operation_error("copy directory", source, source_error))?;
        fs::rename(staging.path(), destination).map_err(|source_error| {
            operation_error("publish copied directory", destination, source_error)
        })?;
        staging.disarm();

        fs::remove_dir_all(source)
            .map_err(|source_error| operation_error("remove copied source", source, source_error))
    }
}

fn ensure_destination_absent(destination: &Path) -> Result<(), ManagedFileSystemError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(operation_error(
            "move directory",
            destination,
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(operation_error(
            "inspect move destination",
            destination,
            source,
        )),
    }
}

fn copy_directory_contents(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            fs::create_dir(&destination_path)?;
            copy_directory_contents(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        } else if file_type.is_symlink() {
            copy_symbolic_link(&source_path, &destination_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported directory entry: {}", source_path.display()),
            ));
        }
    }

    Ok(())
}

#[cfg(unix)]
fn copy_symbolic_link(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_symbolic_link(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    if fs::metadata(source)?.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("\\\\")
        || path
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
}

fn uses_windows_syntax(path: &str) -> bool {
    path.starts_with("\\\\")
        || path.contains('\\')
        || path
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
}

fn normalize_unix_path(folder_path: &str, file_path: &str) -> String {
    let absolute = folder_path.starts_with('/');
    let mut components = Vec::new();
    push_normalized_components(&mut components, folder_path.split('/'));
    push_normalized_components(&mut components, file_path.split('/'));

    let joined = components.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

fn normalize_windows_path(folder_path: &str, file_path: &str) -> String {
    let (root, folder_remainder) = windows_root(folder_path);
    let mut components = Vec::new();
    push_normalized_components(&mut components, folder_remainder.split(['/', '\\']));
    push_normalized_components(&mut components, file_path.split(['/', '\\']));

    match (root, components.is_empty()) {
        (Some(root), true) => format!("{root}\\"),
        (Some(root), false) => format!("{root}\\{}", components.join("\\")),
        (None, _) => components.join("\\"),
    }
}

fn windows_root(path: &str) -> (Option<&str>, &str) {
    if path.as_bytes().get(1) == Some(&b':') {
        return (Some(&path[..2]), &path[2..]);
    }

    if let Some(remainder) = path.strip_prefix("\\\\") {
        let mut separator_indices = remainder.match_indices(['/', '\\']);
        if separator_indices.next().is_some()
            && let Some((share_end, _)) = separator_indices.next()
        {
            let root_end = 2 + share_end;
            return (Some(&path[..root_end]), &path[root_end..]);
        }
        return (Some(path.trim_end_matches(['/', '\\'])), "");
    }

    (None, path)
}

fn push_normalized_components<'a>(
    components: &mut Vec<&'a str>,
    candidates: impl IntoIterator<Item = &'a str>,
) {
    for candidate in candidates {
        match candidate {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(candidate),
        }
    }
}

fn is_invalid_filename_character(character: char) -> bool {
    character <= '\u{1f}'
        || matches!(
            character,
            '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/'
        )
}

fn operation_error(
    operation: &'static str,
    path: impl Into<PathBuf>,
    source: io::Error,
) -> ManagedFileSystemError {
    ManagedFileSystemError {
        operation,
        path: path.into(),
        source,
    }
}

struct StagingDirectory {
    path: PathBuf,
    armed: bool,
}

impl StagingDirectory {
    fn create(parent: &Path) -> Result<Self, ManagedFileSystemError> {
        for _ in 0..8 {
            let path = parent.join(format!(".jellyfin-move-{}", Uuid::new_v4().simple()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, armed: true }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(operation_error(
                        "create move staging directory",
                        path,
                        source,
                    ));
                }
            }
        }

        Err(operation_error(
            "create move staging directory",
            parent,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate a unique staging directory",
            ),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
