use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use jellyfin_model::{
    CountryInfo, CultureDto, LocalizationOption, ParentalRating, ParentalRatingScore,
};
use jellyfin_naming::{LanguageInfo, LocalizationManager};
use serde::Deserialize;

const COUNTRIES: &str = include_str!("../resources/localization/countries.json");
const CULTURES: &str = include_str!("../resources/localization/iso6392.txt");
const RATINGS: &str = include_str!("../resources/localization/ratings.json");
const UI_CULTURES: &str = include_str!("../resources/localization/ui_cultures.json");
const UI_STRINGS: &str = include_str!("../resources/localization/ui_strings.json");
const DEFAULT_UI_CULTURE: &str = "en-US";
const UNRATED_VALUES: &[&str] = &["n/a", "unrated", "not rated", "nr"];

type UiResources = HashMap<String, HashMap<String, String>>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatingSystem {
    country_code: String,
    ratings: Vec<RatingEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatingEntry {
    rating_strings: Vec<String>,
    rating_score: ParentalRatingScore,
}

#[derive(Debug, Deserialize)]
struct UiCulture {
    name: String,
    value: String,
    supported: bool,
}

struct UiLocalizationData {
    options: Vec<LocalizationOption>,
    supported_cultures: Vec<String>,
    bcp47_to_resource: Vec<(String, usize)>,
}

enum SeparatorResolution {
    NotHandled,
    Handled(Option<ParentalRatingScore>),
}

/// Immutable access to Jellyfin's embedded globalization resources.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalizationService;

impl LocalizationService {
    #[must_use]
    pub fn countries(&self) -> &'static [CountryInfo] {
        countries()
    }

    /// Returns every ISO 639-2 row, including cultures with duplicate display names.
    #[must_use]
    pub fn cultures(&self) -> &'static [CultureDto] {
        cultures()
    }

    /// Returns cultures in the form used by Jellyfin's API and metadata editor.
    #[must_use]
    pub fn distinct_sorted_cultures(&self) -> &'static [CultureDto] {
        distinct_sorted_cultures()
    }

    #[must_use]
    pub fn try_get_iso6392_t_from_b(&self, iso_b: &str) -> Option<&'static str> {
        iso6392_b_to_t()
            .iter()
            .find(|(bibliographic, _)| bibliographic.eq_ignore_ascii_case(iso_b))
            .map(|(_, terminologic)| terminologic.as_str())
    }

    #[must_use]
    pub fn find_language_info(&self, language: &str) -> Option<&'static CultureDto> {
        if language.is_empty() {
            return None;
        }

        cultures().iter().find(|culture| {
            culture.display_name.eq_ignore_ascii_case(language)
                || culture.name.eq_ignore_ascii_case(language)
                || culture
                    .three_letter_iso_language_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(language))
                || culture
                    .two_letter_iso_language_name
                    .eq_ignore_ascii_case(language)
        })
    }

    #[must_use]
    pub fn language_display_name(&self, language: Option<&str>) -> Option<String> {
        let display_name = self
            .find_language_info(language?)
            .map(|culture| culture.display_name.as_str())?;
        Some(
            display_name
                .split([';', ','])
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned(),
        )
    }

    #[must_use]
    pub fn parental_ratings(&self, country_code: &str) -> Vec<ParentalRating> {
        parental_ratings(country_code)
    }

    /// Returns every rating spelling that resolves at or below the requested
    /// parental limit using the server's configured metadata country.
    ///
    /// The list is built from every official rating system, including the
    /// common prefixed and `Rated ...` spellings, so the database filter can
    /// compare inherited item ratings without hard-coding one country.
    #[must_use]
    pub fn parental_rating_names_at_or_below(
        &self,
        max_score: i32,
        max_sub_score: Option<i32>,
        configured_country_code: &str,
    ) -> Vec<String> {
        let mut names = HashSet::new();
        let mut add_if_allowed = |candidate: &str| {
            let Some(score) = self.rating_score(candidate, configured_country_code, None) else {
                return;
            };
            if score_at_or_below(score, max_score, max_sub_score) {
                names.insert(candidate.trim().to_lowercase());
            }
        };

        for system in rating_systems() {
            for entry in &system.ratings {
                for rating in &entry.rating_strings {
                    for candidate in
                        rating_spelling_candidates(rating, system.country_code.as_str())
                    {
                        add_if_allowed(&candidate);
                    }
                }
            }
        }
        for value in 0..=max_score.max(0) {
            add_if_allowed(&value.to_string());
            add_if_allowed(&format!("{value}+"));
        }

        names.into_iter().collect()
    }

    /// Returns every embedded server UI culture ordered by native display name.
    #[must_use]
    pub fn localization_options(&self) -> &'static [LocalizationOption] {
        &ui_localization_data().options
    }

    /// Returns embedded UI cultures that can be represented as BCP-47 codes.
    #[must_use]
    pub fn supported_ui_cultures(&self) -> &'static [String] {
        &ui_localization_data().supported_cultures
    }

    /// Looks up a UI phrase for a current culture. An absent culture selects
    /// the configured server culture; a missing translation falls back to `en-US`.
    #[must_use]
    pub fn localized_string(
        &self,
        phrase: &str,
        culture: Option<&str>,
        server_culture: &str,
    ) -> String {
        let culture = culture
            .filter(|culture| !culture.is_empty())
            .unwrap_or(server_culture);
        let culture = if culture.is_empty() {
            DEFAULT_UI_CULTURE
        } else {
            culture
        };
        if let Some(value) = find_ui_string(culture, phrase) {
            return value.to_owned();
        }
        if !culture.eq_ignore_ascii_case(DEFAULT_UI_CULTURE)
            && let Some(value) = find_ui_string(DEFAULT_UI_CULTURE, phrase)
        {
            return value.to_owned();
        }
        phrase.to_owned()
    }

    /// Looks up a UI phrase using the configured server culture.
    #[must_use]
    pub fn server_localized_string(&self, phrase: &str, server_culture: &str) -> String {
        self.localized_string(phrase, None, server_culture)
    }

    /// Resolves a provider rating using Jellyfin's configured-country fallback rules.
    ///
    /// `country_code` is an optional per-call override. When absent, the configured
    /// metadata country is checked first.
    #[must_use]
    pub fn rating_score(
        &self,
        rating: &str,
        configured_country_code: &str,
        country_code: Option<&str>,
    ) -> Option<ParentalRatingScore> {
        if rating.is_empty()
            || UNRATED_VALUES
                .iter()
                .any(|value| value.eq_ignore_ascii_case(rating))
        {
            return None;
        }

        rating
            .split('/')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .find_map(|value| {
                self.single_rating_score(value, configured_country_code, country_code)
            })
    }

    fn single_rating_score(
        self,
        rating: &str,
        configured_country_code: &str,
        country_code: Option<&str>,
    ) -> Option<ParentalRatingScore> {
        if UNRATED_VALUES
            .iter()
            .any(|value| value.eq_ignore_ascii_case(rating))
        {
            return None;
        }

        if let Some(value) = parse_rating_as_score(rating) {
            return Some(score(value));
        }

        let rating = replace_ignore_ascii_case(rating, "Rated :", "");
        let rating = replace_ignore_ascii_case(&rating, "Rated:", "");
        let rating = replace_ignore_ascii_case(&rating, "Rated ", "");
        let rating = rating.trim();

        if let Some(country_code) = country_code.filter(|code| !code.is_empty()) {
            if let Some(value) = rating_in_country(country_code, rating) {
                return Some(value);
            }

            if let Some(suffix) = strip_country_prefix(rating, country_code)
                && let Some(value) = rating_in_country(country_code, suffix)
            {
                return Some(value);
            }
        } else if let Some(value) = rating_in_country(configured_country_code, rating) {
            return Some(value);
        }

        if let Some(value) = rating_in_country("us", rating) {
            return Some(value);
        }

        if let Some(value) = rating_systems()
            .iter()
            .find_map(|system| rating_in_system(system, rating))
        {
            return Some(value);
        }

        if let SeparatorResolution::Handled(result) =
            self.rating_score_by_separator(rating, ':', configured_country_code)
        {
            return result;
        }
        if let SeparatorResolution::Handled(result) =
            self.rating_score_by_separator(rating, '-', configured_country_code)
        {
            return result;
        }

        None
    }

    fn rating_score_by_separator(
        self,
        rating: &str,
        separator: char,
        configured_country_code: &str,
    ) -> SeparatorResolution {
        let Some(first_separator) = rating.find(separator) else {
            return SeparatorResolution::NotHandled;
        };
        let last_separator = rating
            .rfind(separator)
            .expect("a separator found from the left must also be found from the right");
        let country_part = rating[..first_separator].trim();
        let rating_part = rating[last_separator + separator.len_utf8()..].trim();
        if rating_part.is_empty() {
            return SeparatorResolution::NotHandled;
        }

        let resolved_country_code = rating_systems()
            .iter()
            .find(|system| system.country_code.eq_ignore_ascii_case(country_part))
            .map(|system| system.country_code.as_str())
            .or_else(|| {
                self.find_language_info(country_part)
                    .map(|culture| culture.two_letter_iso_language_name.as_str())
                    .filter(|code| !code.is_empty())
                    .and_then(|code| {
                        rating_systems()
                            .iter()
                            .find(|system| system.country_code.eq_ignore_ascii_case(&code))
                            .map(|system| system.country_code.as_str())
                    })
            });

        if let Some(country_code) = resolved_country_code {
            let result = rating_in_country(country_code, rating_part)
                .or_else(|| parse_rating_as_score(rating_part).map(score));
            return SeparatorResolution::Handled(result);
        }

        SeparatorResolution::Handled(self.rating_score(
            rating_part,
            configured_country_code,
            resolved_country_code,
        ))
    }
}

impl LocalizationManager for LocalizationService {
    fn find_language_info(&self, language: &str) -> Option<LanguageInfo> {
        let culture = LocalizationService::find_language_info(self, language)?;
        let three_letter_name = culture
            .three_letter_iso_language_name
            .as_ref()
            .or_else(|| culture.three_letter_iso_language_names.first())
            .cloned();
        Some(LanguageInfo::new(
            culture.display_name.clone(),
            three_letter_name,
        ))
    }
}

fn countries() -> &'static [CountryInfo] {
    static COUNTRIES_CACHE: OnceLock<Vec<CountryInfo>> = OnceLock::new();
    COUNTRIES_CACHE
        .get_or_init(|| serde_json::from_str(COUNTRIES).expect("embedded countries must be valid"))
}

fn cultures() -> &'static [CultureDto] {
    static CULTURES_CACHE: OnceLock<Vec<CultureDto>> = OnceLock::new();
    CULTURES_CACHE.get_or_init(|| {
        CULTURES
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let parts = line.split('|').collect::<Vec<_>>();
                assert_eq!(
                    parts.len(),
                    5,
                    "embedded ISO 639-2 row must have five fields"
                );
                if parts[3].trim().is_empty() {
                    return None;
                }

                let mut name = parts[3].to_owned();
                let two_letter_name = parts[2].to_owned();
                if two_letter_name.contains('-') {
                    name.clone_from(&two_letter_name);
                }
                let mut three_letter_names = vec![parts[0].to_owned()];
                if !parts[1].trim().is_empty() {
                    three_letter_names.push(parts[1].to_owned());
                }
                Some(CultureDto {
                    name,
                    display_name: parts[3].to_owned(),
                    two_letter_iso_language_name: two_letter_name,
                    three_letter_iso_language_name: three_letter_names.first().cloned(),
                    three_letter_iso_language_names: three_letter_names,
                })
            })
            .collect()
    })
}

fn distinct_sorted_cultures() -> &'static [CultureDto] {
    static DISTINCT_CULTURES_CACHE: OnceLock<Vec<CultureDto>> = OnceLock::new();
    DISTINCT_CULTURES_CACHE.get_or_init(|| {
        let mut seen = HashSet::new();
        let mut values = cultures()
            .iter()
            .filter(|culture| seen.insert(culture.display_name.to_lowercase()))
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        values
    })
}

fn iso6392_b_to_t() -> &'static [(String, String)] {
    static ISO6392_B_TO_T_CACHE: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ISO6392_B_TO_T_CACHE.get_or_init(|| {
        CULTURES
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('|');
                let terminologic = parts.next()?;
                let bibliographic = parts.next()?;
                (!bibliographic.trim().is_empty())
                    .then(|| (bibliographic.to_owned(), terminologic.to_owned()))
            })
            .collect()
    })
}

fn rating_systems() -> &'static [RatingSystem] {
    static RATINGS_CACHE: OnceLock<Vec<RatingSystem>> = OnceLock::new();
    RATINGS_CACHE
        .get_or_init(|| serde_json::from_str(RATINGS).expect("embedded ratings must be valid"))
}

fn ui_resources() -> &'static UiResources {
    static UI_RESOURCES_CACHE: OnceLock<UiResources> = OnceLock::new();
    UI_RESOURCES_CACHE.get_or_init(|| {
        let resources: UiResources =
            serde_json::from_str(UI_STRINGS).expect("embedded UI strings must be valid");
        resources
            .into_iter()
            .map(|(culture, dictionary)| {
                let dictionary = dictionary
                    .into_iter()
                    .map(|(key, value)| (key.to_lowercase(), value))
                    .collect();
                (culture, dictionary)
            })
            .collect()
    })
}

fn ui_localization_data() -> &'static UiLocalizationData {
    static UI_LOCALIZATION_DATA_CACHE: OnceLock<UiLocalizationData> = OnceLock::new();
    UI_LOCALIZATION_DATA_CACHE.get_or_init(|| {
        let cultures: Vec<UiCulture> =
            serde_json::from_str(UI_CULTURES).expect("embedded UI cultures must be valid");
        let resources = ui_resources();
        assert_eq!(
            cultures.len(),
            resources.len(),
            "UI culture manifest and string resources must have the same size"
        );
        assert!(
            cultures
                .iter()
                .all(|culture| resources.contains_key(&culture.value)),
            "every UI culture must have a string dictionary"
        );

        let mut options = cultures
            .into_iter()
            .map(|culture| {
                (
                    LocalizationOption {
                        name: culture.name,
                        value: culture.value,
                    },
                    culture.supported,
                )
            })
            .collect::<Vec<_>>();
        options.sort_by_cached_key(|(option, _)| option.name.to_lowercase());
        let mut supported_cultures = Vec::new();
        let options: Vec<LocalizationOption> = options
            .into_iter()
            .map(|(option, supported)| {
                if supported {
                    supported_cultures.push(option.value.replace('_', "-"));
                }
                option
            })
            .collect();
        let bcp47_to_resource = options
            .iter()
            .enumerate()
            .filter(|(_, option)| option.value.contains('_'))
            .map(|(index, option)| (option.value.replace('_', "-"), index))
            .collect();
        UiLocalizationData {
            options,
            supported_cultures,
            bcp47_to_resource,
        }
    })
}

fn find_ui_string(culture: &str, phrase: &str) -> Option<&'static str> {
    let normalized = normalize_ui_culture(culture);
    ui_resources()
        .get(normalized.as_ref())
        .and_then(|dictionary| dictionary.get(&phrase.to_lowercase()))
        .map(String::as_str)
}

fn normalize_ui_culture(culture: &str) -> Cow<'_, str> {
    let data = ui_localization_data();
    if let Some((_, resource_index)) = data
        .bcp47_to_resource
        .iter()
        .find(|(bcp47, _)| bcp47.eq_ignore_ascii_case(culture))
    {
        return Cow::Borrowed(data.options[*resource_index].value.as_str());
    }

    if let Some(index) = culture.find(['-', '_']) {
        let language = culture[..index].to_lowercase();
        let region = culture[index + 1..].to_uppercase();
        let separator = char::from(culture.as_bytes()[index]);
        Cow::Owned(format!("{language}{separator}{region}"))
    } else {
        Cow::Owned(culture.to_lowercase())
    }
}

fn rating_in_country(country_code: &str, rating: &str) -> Option<ParentalRatingScore> {
    rating_systems()
        .iter()
        .find(|system| system.country_code.eq_ignore_ascii_case(country_code))
        .and_then(|system| rating_in_system(system, rating))
}

fn score_at_or_below(
    score: ParentalRatingScore,
    max_score: i32,
    max_sub_score: Option<i32>,
) -> bool {
    score.score < max_score
        || (score.score == max_score && score.sub_score.unwrap_or(0) <= max_sub_score.unwrap_or(0))
}

fn rating_spelling_candidates(rating: &str, country_code: &str) -> Vec<String> {
    let mut candidates = vec![rating.to_owned()];
    for prefix in ["Rated ", "Rated: ", "Rated : "] {
        candidates.push(format!("{prefix}{rating}"));
    }
    for separator in [": ", ":", "-"] {
        candidates.push(format!("{country_code}{separator}{rating}"));
    }
    candidates
}

fn rating_in_system(system: &RatingSystem, rating: &str) -> Option<ParentalRatingScore> {
    system.ratings.iter().rev().find_map(|entry| {
        entry
            .rating_strings
            .iter()
            .rev()
            .any(|value| value.eq_ignore_ascii_case(rating))
            .then_some(entry.rating_score)
    })
}

fn strip_country_prefix<'a>(rating: &'a str, country_code: &str) -> Option<&'a str> {
    let prefix = rating.get(..country_code.len())?;
    if !prefix.eq_ignore_ascii_case(country_code) {
        return None;
    }
    if !matches!(rating.as_bytes().get(country_code.len()), Some(b'-' | b':')) {
        return None;
    }
    rating.get(country_code.len() + 1..).map(str::trim)
}

fn parse_rating_as_score(rating: &str) -> Option<i32> {
    rating.trim_end_matches('+').trim().parse().ok()
}

fn replace_ignore_ascii_case(input: &str, needle: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input
            .get(index..index.saturating_add(needle.len()))
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
        {
            result.push_str(replacement);
            index += needle.len();
        } else {
            let character = input[index..]
                .chars()
                .next()
                .expect("index must remain on a character boundary");
            result.push(character);
            index += character.len_utf8();
        }
    }
    result
}

fn parental_ratings(country_code: &str) -> Vec<ParentalRating> {
    let mut ratings = Vec::new();
    let mut positions = HashMap::new();
    if let Some(system) = rating_systems()
        .iter()
        .find(|system| system.country_code.eq_ignore_ascii_case(country_code))
    {
        for entry in &system.ratings {
            for rating in &entry.rating_strings {
                let normalized = rating.to_lowercase();
                if let Some(position) = positions.get(&normalized).copied() {
                    ratings[position] = ParentalRating::new(rating, Some(entry.rating_score));
                } else {
                    positions.insert(normalized, ratings.len());
                    ratings.push(ParentalRating::new(rating, Some(entry.rating_score)));
                }
            }
        }
    }

    ratings.push(ParentalRating::new("Unrated", None));
    add_score_if_missing(&mut ratings, "Approved", 0);
    add_score_if_missing(&mut ratings, "10", 10);
    add_score_if_missing(&mut ratings, "13", 13);
    add_score_if_missing(&mut ratings, "14", 14);
    if ratings
        .iter()
        .all(|rating| rating.value.is_none_or(|value| value < 21))
    {
        ratings.push(ParentalRating::new("21", Some(score(21))));
    }
    add_score_if_missing(&mut ratings, "XXX", 1000);
    add_score_if_missing(&mut ratings, "Banned", 1001);
    ratings.sort_by_key(|rating| {
        rating
            .rating_score
            .map(|rating_score| (rating_score.score, rating_score.sub_score))
    });
    ratings
}

fn add_score_if_missing(ratings: &mut Vec<ParentalRating>, name: &str, value: i32) {
    if ratings.iter().all(|rating| rating.value != Some(value)) {
        ratings.push(ParentalRating::new(name, Some(score(value))));
    }
}

const fn score(value: i32) -> ParentalRatingScore {
    ParentalRatingScore {
        score: value,
        sub_score: None,
    }
}
