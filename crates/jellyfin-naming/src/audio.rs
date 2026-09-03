use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::NamingOptions;

/// Returns whether `path` points to an audio file recognized by Jellyfin.
#[must_use]
pub fn is_audio_file(path: &str, options: &NamingOptions) -> bool {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return false;
    };
    options
        .audio_file_extensions
        .iter()
        .any(|candidate| candidate[1..].eq_ignore_ascii_case(extension))
}

#[derive(Debug, Clone)]
pub struct AlbumParser {
    options: Arc<NamingOptions>,
}

impl AlbumParser {
    #[must_use]
    pub fn new(options: impl Into<Arc<NamingOptions>>) -> Self {
        Self {
            options: options.into(),
        }
    }

    #[must_use]
    pub fn is_multi_part(&self, path: &str) -> bool {
        let filename = path.rsplit(['/', '\\']).next().unwrap_or(path);
        if filename.is_empty() {
            return false;
        }

        let normalized = CLEAN_EXPRESSION.replace_all(filename, " ");
        let normalized = normalized.trim_start();

        self.options.album_stacking_prefixes.iter().any(|prefix| {
            let Some(candidate) = normalized.get(..prefix.len()) else {
                return false;
            };
            if !candidate.eq_ignore_ascii_case(prefix) {
                return false;
            }

            normalized
                .get(prefix.len()..)
                .map(str::trim)
                .and_then(|remainder| remainder.split(' ').next())
                .is_some_and(|number| number.parse::<i32>().is_ok())
        })
    }
}

static CLEAN_EXPRESSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[-.()\s]+").expect("album multipart clean expression must be valid")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_file_parser_uses_naming_extension_list() {
        let options = NamingOptions::default();
        assert!(is_audio_file("/media/Song.FLAC", &options));
        assert!(is_audio_file("/media/Song.mp3", &options));
        assert!(!is_audio_file("/media/Movie.mkv", &options));
        assert!(!is_audio_file("/media/NoExtension", &options));
    }
}
