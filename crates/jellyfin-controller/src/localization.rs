use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use jellyfin_model::{CountryInfo, CultureDto, ParentalRating, ParentalRatingScore};
use serde::Deserialize;

const COUNTRIES: &str = include_str!("../resources/localization/countries.json");
const CULTURES: &str = include_str!("../resources/localization/iso6392.txt");
const RATINGS: &str = include_str!("../resources/localization/ratings.json");

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

/// Immutable access to Jellyfin's embedded globalization resources.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalizationService;

impl LocalizationService {
    #[must_use]
    pub fn countries(&self) -> Vec<CountryInfo> {
        countries().to_vec()
    }

    #[must_use]
    pub fn cultures(&self) -> Vec<CultureDto> {
        cultures().to_vec()
    }

    #[must_use]
    pub fn parental_ratings(&self, country_code: &str) -> Vec<ParentalRating> {
        parental_ratings(country_code)
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
        let mut seen = HashSet::new();
        let mut cultures = CULTURES
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let parts = line.split('|').collect::<Vec<_>>();
                assert_eq!(
                    parts.len(),
                    5,
                    "embedded ISO 639-2 row must have five fields"
                );
                let mut name = parts[3].to_owned();
                let two_letter_name = parts[2].to_owned();
                if two_letter_name.contains('-') {
                    name.clone_from(&two_letter_name);
                }
                let mut three_letter_names = vec![parts[0].to_owned()];
                if !parts[1].trim().is_empty() {
                    three_letter_names.push(parts[1].to_owned());
                }
                CultureDto {
                    name,
                    display_name: parts[3].to_owned(),
                    two_letter_iso_language_name: two_letter_name,
                    three_letter_iso_language_name: three_letter_names.first().cloned(),
                    three_letter_iso_language_names: three_letter_names,
                }
            })
            .filter(|culture| {
                !culture.display_name.trim().is_empty()
                    && seen.insert(culture.display_name.to_lowercase())
            })
            .collect::<Vec<_>>();
        cultures.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        cultures
    })
}

fn rating_systems() -> &'static [RatingSystem] {
    static RATINGS_CACHE: OnceLock<Vec<RatingSystem>> = OnceLock::new();
    RATINGS_CACHE
        .get_or_init(|| serde_json::from_str(RATINGS).expect("embedded ratings must be valid"))
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
        .all(|rating| rating.value.is_none_or(|score| score < 21))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_localization_is_complete_and_officially_ordered() {
        let service = LocalizationService;
        assert_eq!(service.countries().len(), 140);
        let cultures = service.cultures();
        assert_eq!(cultures.len(), 494);
        assert!(
            cultures
                .windows(2)
                .all(|pair| pair[0].display_name <= pair[1].display_name)
        );
        let ratings = service.parental_ratings("US");
        assert!(ratings.iter().any(|rating| rating.name == "PG-13"));
        assert!(ratings.iter().any(|rating| rating.name == "Banned"));
    }
}
