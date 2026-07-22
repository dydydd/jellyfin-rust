const IGNORED_BASENAMES: &[&str] = &["small.jpg", "albumart.jpg", "thumbs.db"];

const IGNORED_DIRECTORY_NAMES: &[&str] = &[
    "metadata",
    "ps3_update",
    "ps3_vprm",
    "extrafanart",
    "extrathumbs",
    ".actors",
    ".wd_tv",
    "lost+found",
    "subs",
    ".snapshots",
    ".snapshot",
    "temprec",
    "tempsbe",
    "eadir",
    "@eadir",
    "#recycle",
    "@recycle",
    ".@__thumb",
    "$recycle.bin",
    "system volume information",
    ".grab",
    ".zfs",
];

/// Matches Jellyfin's fixed library-scan ignore patterns.
#[derive(Debug, Clone, Copy, Default)]
pub struct IgnorePatterns;

impl IgnorePatterns {
    /// Returns whether `path` matches one of Jellyfin's fixed ignore rules.
    #[must_use]
    pub fn should_ignore(path: &str) -> bool {
        let basename = path.rsplit(['/', '\\']).next().unwrap_or_default();

        IGNORED_BASENAMES
            .iter()
            .any(|ignored| basename.eq_ignore_ascii_case(ignored))
            || basename.starts_with('.')
            || has_ignored_extension(basename)
            || is_sample_name(basename, "sample")
            || is_sample_name(basename, "minta")
            || has_direct_sample_parent(path, "sample")
            || has_direct_sample_parent(path, "minta")
            || path.split(['/', '\\']).any(is_ignored_directory)
            || path
                .split(['/', '\\'])
                .any(|segment| ends_with_ignore_ascii_case(segment, ".trickplay"))
    }
}

fn has_ignored_extension(basename: &str) -> bool {
    [".bts", ".sync"]
        .iter()
        .any(|extension| ends_with_ignore_ascii_case(basename, extension))
}

fn is_sample_name(basename: &str, marker: &str) -> bool {
    let Some((stem, extension)) = basename.rsplit_once('.') else {
        return false;
    };
    let extension_len = extension.chars().count();

    (1..=5).contains(&extension_len)
        && (stem.eq_ignore_ascii_case(marker)
            || stem
                .rsplit_once('.')
                .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(marker)))
}

fn has_direct_sample_parent(path: &str, marker: &str) -> bool {
    let mut segments = path.rsplit(['/', '\\']);
    let _child = segments.next().unwrap_or_default();
    let parent = segments.next().unwrap_or_default();

    parent.eq_ignore_ascii_case(marker)
}

fn is_ignored_directory(segment: &str) -> bool {
    IGNORED_DIRECTORY_NAMES
        .iter()
        .any(|ignored| segment.eq_ignore_ascii_case(ignored))
}

fn ends_with_ignore_ascii_case(value: &str, suffix: &str) -> bool {
    value
        .get(value.len().saturating_sub(suffix.len())..)
        .is_some_and(|ending| ending.eq_ignore_ascii_case(suffix))
}
