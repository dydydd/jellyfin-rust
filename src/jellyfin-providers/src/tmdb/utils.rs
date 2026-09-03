/// Pure utility functions shared by `TMDb` metadata providers.
#[derive(Clone, Copy, Debug, Default)]
pub struct TmdbUtils;

impl TmdbUtils {
    pub const BASE_TMDB_URL: &'static str = "https://www.themoviedb.org/";
    pub const IMAGE_BASE_URL: &'static str = "https://image.tmdb.org/t/p/";
    pub const PROVIDER_NAME: &'static str = "TheMovieDb";

    /// Normalizes a language code for `TMDb`'s language parameters.
    #[must_use]
    pub fn normalize_language(
        language: Option<&str>,
        country_code: Option<&str>,
    ) -> Option<String> {
        let language = language?;
        if language.is_empty() {
            return Some(String::new());
        }

        let language = if language.eq_ignore_ascii_case("es-419")
            && country_code.is_some_and(|country| !country.is_empty())
        {
            if country_code.is_some_and(|country| country.eq_ignore_ascii_case("AR")) {
                "es-AR"
            } else {
                "es-MX"
            }
        } else {
            language
        };

        let mut parts = language.split('-');
        let first = parts.next().unwrap_or_default();
        let second = parts.next();
        if let Some(region) = second
            && parts.next().is_none()
        {
            if region.eq_ignore_ascii_case("CH") {
                return Some(first.to_owned());
            }
            return Some(format!("{first}-{}", region.to_uppercase()));
        }

        Some(language.to_owned())
    }

    /// Builds `TMDb`'s comma-separated include-image-language parameter.
    #[must_use]
    pub fn image_languages_param(preferred_language: &str, country_code: Option<&str>) -> String {
        let normalized = if preferred_language.is_empty() {
            None
        } else {
            Self::normalize_language(Some(preferred_language), country_code)
        };
        let mut languages = Vec::with_capacity(3);
        if let Some(language) = normalized.as_deref() {
            languages.push(language);
        }
        languages.push("null");
        if normalized
            .as_deref()
            .is_none_or(|language| !language.eq_ignore_ascii_case("en"))
        {
            languages.push("en");
        }
        languages.join(",")
    }

    /// Prefers the requested regional image language when the API only returns a base language.
    #[must_use]
    pub fn adjust_image_language(image_language: Option<&str>, request_language: &str) -> String {
        let Some(image_language) = image_language else {
            return String::new();
        };
        if image_language.is_empty() {
            return String::new();
        }
        if !request_language.is_empty()
            && request_language.len() > 2
            && image_language.len() == 2
            && starts_with_ignore_ascii_case(request_language, image_language)
        {
            return request_language.to_owned();
        }
        if image_language.eq_ignore_ascii_case("xx") {
            String::new()
        } else {
            image_language.to_owned()
        }
    }

    /// Replaces punctuation with spaces as required by `TMDb` searches.
    #[must_use]
    pub fn clean_name(name: &str) -> String {
        let mut cleaned = String::with_capacity(name.len());
        let mut replacing = false;
        for character in name.chars() {
            if character.is_alphanumeric() || character == '·' {
                cleaned.push(character);
                replacing = false;
            } else if !replacing {
                cleaned.push(' ');
                replacing = true;
            }
        }
        cleaned
    }

    /// Maps a `TMDb` crew department and job to the Jellyfin person kind.
    #[must_use]
    pub fn map_crew_to_person_kind(department: Option<&str>, job: Option<&str>) -> TmdbPersonKind {
        if equals_ignore_ascii_case(department, "directing")
            && equals_ignore_ascii_case(job, "director")
        {
            return TmdbPersonKind::Director;
        }
        if equals_ignore_ascii_case(department, "production")
            && equals_ignore_ascii_case(job, "producer")
        {
            return TmdbPersonKind::Producer;
        }
        if equals_ignore_ascii_case(department, "writing")
            && job.is_some_and(|job| {
                ["writer", "screenplay", "novel"]
                    .iter()
                    .any(|candidate| job.eq_ignore_ascii_case(candidate))
            })
        {
            return TmdbPersonKind::Writer;
        }
        TmdbPersonKind::Unknown
    }

    /// Returns whether a `TMDb` video is a `YouTube` trailer or teaser.
    #[must_use]
    pub fn is_trailer_type(site: Option<&str>, video_type: Option<&str>) -> bool {
        equals_ignore_ascii_case(site, "youtube")
            && (equals_ignore_ascii_case(video_type, "trailer")
                || equals_ignore_ascii_case(video_type, "teaser"))
    }

    /// Combines a country code and API rating in Jellyfin's parental-rating format.
    #[must_use]
    pub fn build_parental_rating(country_code: &str, rating_value: &str) -> String {
        let rating = if country_code.eq_ignore_ascii_case("US") {
            rating_value.to_owned()
        } else {
            format!("{country_code}-{rating_value}")
        };
        replace_ignore_ascii_case(&rating, "DE-", "FSK-")
    }

    /// Builds an absolute `TMDb` image URL from a configured size and relative path.
    #[must_use]
    pub fn image_url(size: Option<&str>, path: Option<&str>) -> Option<String> {
        let path = path.filter(|path| !path.is_empty())?;
        let size = size.filter(|size| !size.is_empty()).unwrap_or("original");
        Some(format!(
            "{}{size}/{}",
            Self::IMAGE_BASE_URL,
            path.trim_start_matches('/')
        ))
    }
}

/// Jellyfin person kinds retained from `TMDb` crew responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TmdbPersonKind {
    Director,
    Writer,
    Producer,
    Unknown,
}

fn equals_ignore_ascii_case(value: Option<&str>, expected: &str) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn replace_ignore_ascii_case(value: &str, needle: &str, replacement: &str) -> String {
    let lowercase = value.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut result = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_index) = lowercase[cursor..].find(&needle) {
        let index = cursor + relative_index;
        result.push_str(&value[cursor..index]);
        result.push_str(replacement);
        cursor = index + needle.len();
    }
    result.push_str(&value[cursor..]);
    result
}
