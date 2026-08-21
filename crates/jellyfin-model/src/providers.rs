use std::{cmp::Ordering, collections::HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ImageType {
    #[default]
    Primary = 0,
    Art = 1,
    Backdrop = 2,
    Banner = 3,
    Logo = 4,
    Thumb = 5,
    Disc = 6,
    Box = 7,
    Screenshot = 8,
    Menu = 9,
    Chapter = 10,
    BoxRear = 11,
    Profile = 12,
}

/// Metadata describing one image attached to a library item.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ImageInfo {
    pub image_type: ImageType,
    pub image_index: Option<i32>,
    pub image_tag: String,
    pub path: String,
    pub blur_hash: Option<String>,
    pub height: Option<i32>,
    pub width: Option<i32>,
    pub size: i64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum RatingType {
    #[default]
    Score = 0,
    Likes = 1,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteImageInfo {
    pub provider_name: Option<String>,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub height: Option<i32>,
    pub width: Option<i32>,
    pub community_rating: Option<f64>,
    pub vote_count: Option<i32>,
    pub language: Option<String>,
    #[serde(rename = "Type")]
    pub image_type: ImageType,
    pub rating_type: RatingType,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteImageResult {
    pub images: Vec<RemoteImageInfo>,
    pub total_record_count: i32,
    pub providers: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ImageProviderInfo {
    pub name: String,
    pub supported_images: Vec<ImageType>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteSubtitleInfo {
    #[serde(rename = "ThreeLetterISOLanguageName")]
    pub three_letter_iso_language_name: Option<String>,
    pub id: Option<String>,
    pub provider_name: Option<String>,
    pub name: Option<String>,
    pub format: Option<String>,
    pub author: Option<String>,
    pub comment: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub date_created: Option<DateTime<Utc>>,
    pub community_rating: Option<f32>,
    pub frame_rate: Option<f32>,
    pub download_count: Option<i32>,
    pub is_hash_match: Option<bool>,
    pub ai_translated: Option<bool>,
    pub machine_translated: Option<bool>,
    pub forced: Option<bool>,
    pub hearing_impaired: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteSearchResult {
    pub name: Option<String>,
    #[serde(rename = "Type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub provider_ids: HashMap<String, String>,
    pub production_year: Option<i32>,
    pub index_number: Option<i32>,
    pub index_number_end: Option<i32>,
    pub parent_index_number: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub premiere_date: Option<DateTime<Utc>>,
    pub image_url: Option<String>,
    pub search_provider_name: Option<String>,
    pub overview: Option<String>,
    pub album_artist: Option<Box<RemoteSearchResult>>,
    pub artists: Vec<RemoteSearchResult>,
}

/// Orders remote images by Jellyfin's language, rating, and vote priorities.
#[must_use]
pub fn order_by_language_descending<I>(
    images: I,
    requested_language: Option<&str>,
) -> Vec<RemoteImageInfo>
where
    I: IntoIterator<Item = RemoteImageInfo>,
{
    let requested_language = requested_language
        .filter(|language| !language.trim().is_empty())
        .unwrap_or("en");
    let mut images = images.into_iter().collect::<Vec<_>>();
    images.sort_by(|left, right| {
        language_priority(right.language.as_deref(), requested_language)
            .cmp(&language_priority(
                left.language.as_deref(),
                requested_language,
            ))
            .then_with(|| {
                rounded_rating(right.community_rating)
                    .partial_cmp(&rounded_rating(left.community_rating))
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| {
                right
                    .vote_count
                    .unwrap_or_default()
                    .cmp(&left.vote_count.unwrap_or_default())
            })
    });
    images
}

fn language_priority(language: Option<&str>, requested_language: &str) -> u8 {
    if language.is_some_and(|language| language.eq_ignore_ascii_case(requested_language)) {
        4
    } else if language.is_some_and(|language| language.eq_ignore_ascii_case("en")) {
        3
    } else if language.is_none_or(str::is_empty) {
        2
    } else {
        0
    }
}

fn rounded_rating(rating: Option<f64>) -> f64 {
    (rating.unwrap_or_default() * 10.0).round_ties_even() / 10.0
}
