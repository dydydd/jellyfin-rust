//! Host-independent operations on Jellyfin path strings.

use std::{error::Error, fmt, path::MAIN_SEPARATOR};

const BACKSLASH: char = '\\';
const FORWARD_SLASH: char = '/';

/// Error returned when path normalization receives a non-directory separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPathSeparator {
    separator: char,
}

impl InvalidPathSeparator {
    /// Returns the invalid separator supplied by the caller.
    #[must_use]
    pub const fn separator(self) -> char {
        self.separator
    }
}

impl fmt::Display for InvalidPathSeparator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "'{}' is not a directory separator; expected '/' or '\\\\'",
            self.separator
        )
    }
}

impl Error for InvalidPathSeparator {}

/// Replaces a leading directory path using Jellyfin's path-string semantics.
///
/// Both slash styles are accepted regardless of the host operating system.
/// Matching is case-insensitive and requires a complete directory component,
/// so replacing `/media/tv` cannot accidentally match `/media/tv-old`.
/// `None`, empty inputs, and paths outside `sub_path` return `None`.
#[must_use]
pub fn try_replace_sub_path(
    path: Option<&str>,
    sub_path: Option<&str>,
    new_sub_path: Option<&str>,
) -> Option<String> {
    let (path, sub_path, new_sub_path) = (path?, sub_path?, new_sub_path?);
    if path.is_empty() || sub_path.is_empty() || new_sub_path.is_empty() {
        return None;
    }

    let (sub_path, separator) = normalize_path_with_detected_separator(Some(sub_path))?;
    let separator = separator.expect("a non-empty subpath always has a detected separator");
    let path = normalize_path(Some(path), separator)
        .expect("a detected separator is always valid")
        .expect("the input path is non-null");
    let prefix_end = ordinal_ignore_case_prefix_len(&path, &sub_path)?;
    let old_sub_path_ends_with_separator = sub_path.ends_with(separator);

    if path
        .get(prefix_end..)
        .is_some_and(|suffix| !suffix.is_empty())
        && !old_sub_path_ends_with_separator
        && !path[prefix_end..].starts_with(separator)
    {
        return None;
    }

    let suffix_start = if old_sub_path_ends_with_separator {
        prefix_end - separator.len_utf8()
    } else {
        prefix_end
    };
    let new_sub_path = new_sub_path.trim_end_matches(separator);
    Some(format!("{new_sub_path}{}", &path[suffix_start..]))
}

/// Normalizes both directory separator styles to `separator`.
///
/// A null path remains `None`, while an empty path remains `Some("")`.
/// This function only transforms strings and never asks the host filesystem to
/// interpret Windows or Unix path syntax.
///
/// # Errors
///
/// Returns [`InvalidPathSeparator`] unless `separator` is `/` or `\\`.
pub fn normalize_path(
    path: Option<&str>,
    separator: char,
) -> Result<Option<String>, InvalidPathSeparator> {
    if !matches!(separator, FORWARD_SLASH | BACKSLASH) {
        return Err(InvalidPathSeparator { separator });
    }

    Ok(path.map(|path| match separator {
        FORWARD_SLASH => path.replace(BACKSLASH, "/"),
        BACKSLASH => path.replace(FORWARD_SLASH, "\\"),
        _ => unreachable!("separator was validated"),
    }))
}

/// Normalizes a path to the host platform's directory separator.
#[must_use]
pub fn normalize_path_default(path: Option<&str>) -> Option<String> {
    normalize_path(path, MAIN_SEPARATOR).expect("the host directory separator is valid")
}

/// Normalizes a path and returns the separator inferred from its spelling.
///
/// A forward slash anywhere in the path selects `/`; otherwise `\\` is used.
/// Null input returns `None`. Empty input is returned with no detected
/// separator, mirroring Jellyfin's default out-parameter value.
#[must_use]
pub fn normalize_path_with_detected_separator(
    path: Option<&str>,
) -> Option<(String, Option<char>)> {
    let path = path?;
    if path.is_empty() {
        return Some((String::new(), None));
    }

    let separator = if path.contains(FORWARD_SLASH) {
        FORWARD_SLASH
    } else {
        BACKSLASH
    };
    let normalized = normalize_path(Some(path), separator)
        .expect("the detected directory separator is valid")
        .expect("the input path is non-null");
    Some((normalized, Some(separator)))
}

fn ordinal_ignore_case_prefix_len(text: &str, prefix: &str) -> Option<usize> {
    let mut text_chars = text.char_indices();

    for expected in prefix.chars() {
        let (_, actual) = text_chars.next()?;
        if actual != expected && !actual.to_uppercase().eq(expected.to_uppercase()) {
            return None;
        }
    }

    Some(text_chars.next().map_or(text.len(), |(index, _)| index))
}
