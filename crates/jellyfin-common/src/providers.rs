//! Parsers for provider identifiers embedded in text, paths, and URLs.

use std::{error::Error, fmt};

const IMDB_MIN_DIGITS: usize = 7;
const IMDB_MAX_DIGITS: usize = 8;
const IMDB_PREFIX: &str = "tt";
const TMDB_MOVIE_PATH: &str = "themoviedb.org/movie/";
const TMDB_SERIES_PATH: &str = "themoviedb.org/tv/";
const TVDB_SERIES_PATH: &str = "thetvdb.com/?tab=series&id=";

/// Namespace-compatible provider identifier parsers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderIdParsers;

/// Identifies which argument to [`get_attribute_value`] was invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeValueInput {
    Text,
    Attribute,
}

/// Error returned when a path-attribute lookup receives an empty argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeValueError {
    InvalidInput(AttributeValueInput),
}

impl fmt::Display for AttributeValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(AttributeValueInput::Text) => {
                formatter.write_str("text cannot be empty")
            }
            Self::InvalidInput(AttributeValueInput::Attribute) => {
                formatter.write_str("attribute cannot be empty")
            }
        }
    }
}

impl Error for AttributeValueError {}

impl ProviderIdParsers {
    /// Finds a bracketed provider attribute in a media path or name.
    ///
    /// # Errors
    ///
    /// Returns [`AttributeValueError::InvalidInput`] when either argument is
    /// empty.
    pub fn get_attribute_value<'a>(
        text: &'a str,
        attribute: &str,
    ) -> Result<Option<&'a str>, AttributeValueError> {
        get_attribute_value(text, attribute)
    }

    /// Finds the first valid IMDb identifier in `text`.
    ///
    /// IMDb identifiers contain the lowercase `tt` prefix followed by seven
    /// or eight ASCII digits. Matching the upstream behavior, identifiers
    /// with additional digits are truncated to eight digits.
    #[must_use]
    pub fn find_imdb_id(text: &str) -> Option<&str> {
        find_imdb_id(text)
    }

    /// Alias matching Jellyfin's `TryFindImdbId` operation.
    #[must_use]
    pub fn try_find_imdb_id(text: &str) -> Option<&str> {
        find_imdb_id(text)
    }

    /// Extracts a TMDb movie identifier from a TMDb movie URL.
    #[must_use]
    pub fn find_tmdb_movie_id(text: &str) -> Option<&str> {
        find_tmdb_movie_id(text)
    }

    /// Alias matching Jellyfin's `TryFindTmdbMovieId` operation.
    #[must_use]
    pub fn try_find_tmdb_movie_id(text: &str) -> Option<&str> {
        find_tmdb_movie_id(text)
    }

    /// Extracts a TMDb series identifier from a TMDb TV URL.
    #[must_use]
    pub fn find_tmdb_series_id(text: &str) -> Option<&str> {
        find_tmdb_series_id(text)
    }

    /// Alias matching Jellyfin's `TryFindTmdbSeriesId` operation.
    #[must_use]
    pub fn try_find_tmdb_series_id(text: &str) -> Option<&str> {
        find_tmdb_series_id(text)
    }

    /// Extracts a TVDb series identifier from a legacy TVDb URL.
    #[must_use]
    pub fn find_tvdb_id(text: &str) -> Option<&str> {
        find_tvdb_id(text)
    }

    /// Alias matching Jellyfin's `TryFindTvdbId` operation.
    #[must_use]
    pub fn try_find_tvdb_id(text: &str) -> Option<&str> {
        find_tvdb_id(text)
    }
}

/// Finds a bracketed provider attribute in a media path or name.
///
/// `tmdb`, `tvdb`, and `imdb` are accepted as aliases for their corresponding
/// `*id` attributes. Values may use `=` or `-` and must be enclosed by a
/// matching `[]`, `{}`, or `()` pair.
///
/// # Errors
///
/// Returns [`AttributeValueError::InvalidInput`] when either argument is
/// empty.
pub fn get_attribute_value<'a>(
    text: &'a str,
    attribute: &str,
) -> Result<Option<&'a str>, AttributeValueError> {
    if text.is_empty() {
        return Err(AttributeValueError::InvalidInput(AttributeValueInput::Text));
    }
    if attribute.is_empty() {
        return Err(AttributeValueError::InvalidInput(
            AttributeValueInput::Attribute,
        ));
    }

    let short_attribute = if attribute.eq_ignore_ascii_case("tmdbid") {
        Some("tmdb")
    } else if attribute.eq_ignore_ascii_case("tvdbid") {
        Some("tvdb")
    } else if attribute.eq_ignore_ascii_case("imdbid") {
        Some("imdb")
    } else {
        None
    };
    let search_attribute = short_attribute.unwrap_or(attribute);
    let mut search_start = 0;

    while search_start < text.len() {
        let remainder = &text[search_start..];
        let Some(attribute_index) = find_ignore_ascii_case(remainder, search_attribute) else {
            break;
        };
        let mut attribute_end = attribute_index + search_attribute.len();
        search_start += attribute_end;

        if attribute_index == 0 {
            continue;
        }
        let opener = remainder.as_bytes()[attribute_index - 1];
        let closer = match opener {
            b'[' => b']',
            b'(' => b')',
            b'{' => b'}',
            _ => continue,
        };

        if short_attribute.is_some()
            && remainder
                .as_bytes()
                .get(attribute_end..attribute_end + 2)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b"id"))
        {
            attribute_end += 2;
        }

        let bytes = remainder.as_bytes();
        if attribute_end + 2 >= bytes.len() || !matches!(bytes[attribute_end], b'=' | b'-') {
            continue;
        }
        let value_and_closer = &bytes[attribute_end..];
        let Some(closing_index) = value_and_closer.iter().position(|byte| *byte == closer) else {
            continue;
        };
        if closing_index <= 1 {
            continue;
        }

        let value = remainder[attribute_end + 1..attribute_end + closing_index].trim();
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }

    if attribute.eq_ignore_ascii_case("imdbid") {
        return Ok(find_imdb_id(text));
    }
    Ok(None)
}

fn find_ignore_ascii_case(text: &str, needle: &str) -> Option<usize> {
    if needle.len() > text.len() {
        return None;
    }
    text.as_bytes()
        .windows(needle.len())
        .position(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Finds the first valid IMDb identifier in `text`.
#[must_use]
pub fn find_imdb_id(mut text: &str) -> Option<&str> {
    // The shortest possible identifier is "tt" plus seven digits.
    while text.len() >= IMDB_PREFIX.len() + IMDB_MIN_DIGITS {
        let prefix_position = text.find(IMDB_PREFIX)?;
        text = &text[prefix_position..];

        let digit_count = text.as_bytes()[IMDB_PREFIX.len()..]
            .iter()
            .take(IMDB_MAX_DIGITS)
            .take_while(|byte| byte.is_ascii_digit())
            .count();

        if digit_count >= IMDB_MIN_DIGITS {
            return Some(&text[..IMDB_PREFIX.len() + digit_count]);
        }

        // Both prefix bytes and every consumed digit are ASCII, so this is a
        // valid UTF-8 boundary and guarantees progress to the next candidate.
        text = &text[IMDB_PREFIX.len() + digit_count..];
    }

    None
}

/// Alias matching Jellyfin's `TryFindImdbId` operation.
#[must_use]
pub fn try_find_imdb_id(text: &str) -> Option<&str> {
    find_imdb_id(text)
}

/// Extracts a TMDb movie identifier from a TMDb movie URL.
#[must_use]
pub fn find_tmdb_movie_id(text: &str) -> Option<&str> {
    find_provider_id(text, TMDB_MOVIE_PATH)
}

/// Alias matching Jellyfin's `TryFindTmdbMovieId` operation.
#[must_use]
pub fn try_find_tmdb_movie_id(text: &str) -> Option<&str> {
    find_tmdb_movie_id(text)
}

/// Extracts a TMDb series identifier from a TMDb TV URL.
#[must_use]
pub fn find_tmdb_series_id(text: &str) -> Option<&str> {
    find_provider_id(text, TMDB_SERIES_PATH)
}

/// Alias matching Jellyfin's `TryFindTmdbSeriesId` operation.
#[must_use]
pub fn try_find_tmdb_series_id(text: &str) -> Option<&str> {
    find_tmdb_series_id(text)
}

/// Extracts a TVDb series identifier from a legacy TVDb URL.
#[must_use]
pub fn find_tvdb_id(text: &str) -> Option<&str> {
    find_provider_id(text, TVDB_SERIES_PATH)
}

/// Alias matching Jellyfin's `TryFindTvdbId` operation.
#[must_use]
pub fn try_find_tvdb_id(text: &str) -> Option<&str> {
    find_tvdb_id(text)
}

fn find_provider_id<'a>(text: &'a str, search: &str) -> Option<&'a str> {
    let id_start = text.find(search)? + search.len();
    let remainder = &text[id_start..];
    let digit_count = remainder
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();

    (digit_count > 0).then(|| &remainder[..digit_count])
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderIdParsers, find_imdb_id, find_tmdb_movie_id, find_tmdb_series_id, find_tvdb_id,
    };

    #[test]
    fn finds_official_imdb_examples() {
        let cases = [
            ("tt1234567", "tt1234567"),
            ("tt12345678", "tt12345678"),
            ("https://www.imdb.com/title/tt1234567", "tt1234567"),
            ("https://www.imdb.com/title/tt12345678", "tt12345678"),
            (
                r"multiline\nhttps://www.imdb.com/title/tt1234567",
                "tt1234567",
            ),
            (
                r"multiline\nhttps://www.imdb.com/title/tt12345678",
                "tt12345678",
            ),
            ("tt1234567tt7654321", "tt1234567"),
            ("tt12345678tt7654321", "tt12345678"),
            ("tt123456789", "tt12345678"),
        ];

        for (text, expected) in cases {
            assert_eq!(Some(expected), ProviderIdParsers::find_imdb_id(text));
        }
    }

    #[test]
    fn rejects_official_invalid_imdb_examples() {
        for text in [
            "tt123456",
            "https://www.imdb.com/title/tt123456",
            "Jellyfin",
        ] {
            assert_eq!(None, find_imdb_id(text));
        }
    }

    #[test]
    fn skips_short_imdb_candidate_and_finds_later_valid_one() {
        assert_eq!(Some("tt7654321"), find_imdb_id("tt123 then tt7654321"));
    }

    #[test]
    fn imdb_matching_is_ascii_and_case_sensitive() {
        assert_eq!(None, find_imdb_id("TT1234567"));
        assert_eq!(None, find_imdb_id("tt１２３４５６７"));
    }

    #[test]
    fn finds_official_tmdb_movie_examples() {
        assert_eq!(
            Some("30287"),
            find_tmdb_movie_id("https://www.themoviedb.org/movie/30287-fallo")
        );
        assert_eq!(
            Some("30287"),
            find_tmdb_movie_id("themoviedb.org/movie/30287")
        );
    }

    #[test]
    fn rejects_official_invalid_tmdb_movie_examples() {
        assert_eq!(
            None,
            find_tmdb_movie_id("https://www.themoviedb.org/movie/fallo-30287")
        );
        assert_eq!(
            None,
            find_tmdb_movie_id("https://www.themoviedb.org/tv/1668-friends")
        );
    }

    #[test]
    fn finds_official_tmdb_series_examples() {
        assert_eq!(
            Some("1668"),
            find_tmdb_series_id("https://www.themoviedb.org/tv/1668-friends")
        );
        assert_eq!(Some("1668"), find_tmdb_series_id("themoviedb.org/tv/1668"));
    }

    #[test]
    fn rejects_official_invalid_tmdb_series_examples() {
        assert_eq!(
            None,
            find_tmdb_series_id("https://www.themoviedb.org/tv/friends-1668")
        );
        assert_eq!(
            None,
            find_tmdb_series_id("https://www.themoviedb.org/movie/30287-fallo")
        );
    }

    #[test]
    fn finds_official_tvdb_examples() {
        assert_eq!(
            Some("121361"),
            find_tvdb_id("https://www.thetvdb.com/?tab=series&id=121361")
        );
        assert_eq!(
            Some("121361"),
            ProviderIdParsers::find_tvdb_id("thetvdb.com/?tab=series&id=121361")
        );
    }

    #[test]
    fn rejects_official_invalid_tvdb_examples() {
        assert_eq!(
            None,
            find_tvdb_id("thetvdb.com/?tab=series&id=Jellyfin121361")
        );
        assert_eq!(
            None,
            find_tvdb_id("https://www.themoviedb.org/tv/1668-friends")
        );
    }

    #[test]
    fn provider_parser_uses_first_matching_path_like_upstream() {
        assert_eq!(
            None,
            find_tmdb_movie_id("themoviedb.org/movie/no-id then themoviedb.org/movie/30287")
        );
    }
}
