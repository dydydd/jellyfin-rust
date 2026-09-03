//! Helpers for safely composing filesystem paths from untrusted input.

use std::{
    io,
    path::{Component, Path, PathBuf},
};

/// Namespace-compatible entry point for Jellyfin path helpers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathHelper;

impl PathHelper {
    /// Reduces an untrusted filename to a usable leaf component.
    #[must_use]
    pub fn get_safe_leaf_file_name(file_name: &str) -> Option<&str> {
        get_safe_leaf_file_name(file_name)
    }

    /// Checks whether `candidate` is equal to or lexically inside `root`.
    pub fn is_contained_in(
        root: impl AsRef<Path>,
        candidate: impl AsRef<Path>,
    ) -> io::Result<bool> {
        is_contained_in(root, candidate)
    }
}

/// Reduces an untrusted filename to a usable leaf component.
///
/// The returned slice borrows from `file_name`; no filesystem access occurs.
#[must_use]
pub fn get_safe_leaf_file_name(file_name: &str) -> Option<&str> {
    if file_name.is_empty() || ends_with_separator(file_name) {
        return None;
    }

    let leaf = Path::new(file_name).file_name()?.to_str()?;
    (!leaf.is_empty() && leaf != "." && leaf != "..").then_some(leaf)
}

/// Checks whether `candidate` is equal to or lexically inside `root`.
///
/// Both arguments are converted to absolute paths and their `.` and `..`
/// components are normalized. Symlinks are deliberately not resolved, which
/// matches `.NET Path.GetFullPath` and allows paths that do not yet exist.
pub fn is_contained_in(root: impl AsRef<Path>, candidate: impl AsRef<Path>) -> io::Result<bool> {
    let root = absolute_normalized(root.as_ref())?;
    let candidate = absolute_normalized(candidate.as_ref())?;
    Ok(candidate.starts_with(root))
}

fn ends_with_separator(path: &str) -> bool {
    path.ends_with(std::path::MAIN_SEPARATOR)
        || (std::path::MAIN_SEPARATOR != '/' && path.ends_with('/'))
}

fn absolute_normalized(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path cannot be empty",
        ));
    }

    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{PathHelper, get_safe_leaf_file_name, is_contained_in};

    #[test]
    fn safe_leaf_reduces_official_examples() {
        assert_eq!(Some("file.txt"), get_safe_leaf_file_name("file.txt"));
        assert_eq!(
            Some("file.txt"),
            PathHelper::get_safe_leaf_file_name("sub/file.txt")
        );
        assert_eq!(Some("passwd"), get_safe_leaf_file_name("../../etc/passwd"));
    }

    #[test]
    fn safe_leaf_rejects_official_unusable_examples() {
        for input in ["", ".", ".."] {
            assert_eq!(None, get_safe_leaf_file_name(input));
        }
    }

    #[test]
    fn safe_leaf_rejects_trailing_separator() {
        assert_eq!(None, get_safe_leaf_file_name("directory/"));
    }

    #[test]
    fn child_path_is_contained() {
        let root = temp_path("root");
        let child = root.join("sub").join("file.txt");
        assert!(is_contained_in(&root, child).unwrap());
    }

    #[test]
    fn root_is_contained_in_itself() {
        let root = temp_path("root");
        assert!(PathHelper::is_contained_in(&root, &root).unwrap());
    }

    #[test]
    fn traversal_escape_is_not_contained() {
        let root = temp_path("root");
        let escape = root.join("..").join("..").join("etc").join("passwd");
        assert!(!is_contained_in(root, escape).unwrap());
    }

    #[test]
    fn sibling_prefix_collision_is_not_contained() {
        let root = temp_path("data");
        let sibling = temp_path("dataset").join("file.txt");
        assert!(!is_contained_in(root, sibling).unwrap());
    }

    #[test]
    fn relative_dot_segments_are_normalized() {
        assert!(is_contained_in("target/root", "target/root/sub/../file").unwrap());
        assert!(!is_contained_in("target/root", "target/root/../../escape").unwrap());
    }

    #[test]
    fn empty_paths_return_invalid_input() {
        assert_eq!(
            io::ErrorKind::InvalidInput,
            is_contained_in("", "candidate").unwrap_err().kind()
        );
    }

    fn temp_path(leaf: &str) -> PathBuf {
        std::env::temp_dir().join(leaf)
    }

    use std::io;
}
