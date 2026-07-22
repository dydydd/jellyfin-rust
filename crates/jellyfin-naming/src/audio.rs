use std::sync::LazyLock;

use regex::Regex;

use crate::NamingOptions;

pub struct AlbumParser {
    options: NamingOptions,
}

impl AlbumParser {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
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
