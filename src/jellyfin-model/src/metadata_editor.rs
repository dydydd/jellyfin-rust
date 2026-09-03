use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum CollectionType {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "movies")]
    Movies,
    #[serde(rename = "tvshows")]
    TvShows,
    #[serde(rename = "music")]
    Music,
    #[serde(rename = "musicvideos")]
    MusicVideos,
    #[serde(rename = "trailers")]
    Trailers,
    #[serde(rename = "homevideos")]
    HomeVideos,
    #[serde(rename = "boxsets")]
    BoxSets,
    #[serde(rename = "books")]
    Books,
    #[serde(rename = "photos")]
    Photos,
    #[serde(rename = "livetv")]
    LiveTv,
    #[serde(rename = "playlists")]
    Playlists,
    #[serde(rename = "folders")]
    Folders,
}

impl CollectionType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Movies => "movies",
            Self::TvShows => "tvshows",
            Self::Music => "music",
            Self::MusicVideos => "musicvideos",
            Self::Trailers => "trailers",
            Self::HomeVideos => "homevideos",
            Self::BoxSets => "boxsets",
            Self::Books => "books",
            Self::Photos => "photos",
            Self::LiveTv => "livetv",
            Self::Playlists => "playlists",
            Self::Folders => "folders",
        }
    }
}

impl FromStr for CollectionType {
    type Err = ParseCollectionTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        for collection_type in [
            Self::Unknown,
            Self::Movies,
            Self::TvShows,
            Self::Music,
            Self::MusicVideos,
            Self::Trailers,
            Self::HomeVideos,
            Self::BoxSets,
            Self::Books,
            Self::Photos,
            Self::LiveTv,
            Self::Playlists,
            Self::Folders,
        ] {
            if collection_type.as_str().eq_ignore_ascii_case(value) {
                return Ok(collection_type);
            }
        }
        Err(ParseCollectionTypeError)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseCollectionTypeError;

impl fmt::Display for ParseCollectionTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown Jellyfin collection type")
    }
}

impl std::error::Error for ParseCollectionTypeError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct NameValuePair {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct CountryInfo {
    pub name: String,
    pub display_name: String,
    #[serde(rename = "TwoLetterISORegionName")]
    pub two_letter_iso_region_name: String,
    #[serde(rename = "ThreeLetterISORegionName")]
    pub three_letter_iso_region_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct CultureDto {
    pub name: String,
    pub display_name: String,
    #[serde(rename = "TwoLetterISOLanguageName")]
    pub two_letter_iso_language_name: String,
    #[serde(
        rename = "ThreeLetterISOLanguageName",
        skip_serializing_if = "Option::is_none"
    )]
    pub three_letter_iso_language_name: Option<String>,
    #[serde(rename = "ThreeLetterISOLanguageNames")]
    pub three_letter_iso_language_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub struct ParentalRatingScore {
    #[serde(rename = "score")]
    pub score: i32,
    #[serde(rename = "subScore", skip_serializing_if = "Option::is_none")]
    pub sub_score: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct ParentalRating {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating_score: Option<ParentalRatingScore>,
}

impl ParentalRating {
    #[must_use]
    pub fn new(name: impl Into<String>, rating_score: Option<ParentalRatingScore>) -> Self {
        Self {
            name: name.into(),
            value: rating_score.map(|score| score.score),
            rating_score,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum ExternalIdMediaType {
    Album,
    AlbumArtist,
    Artist,
    BoxSet,
    Episode,
    Movie,
    OtherArtist,
    Person,
    ReleaseGroup,
    Season,
    Series,
    Track,
    Book,
    Recording,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct ExternalIdInfo {
    pub name: String,
    pub key: String,
    #[serde(rename = "Type", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<ExternalIdMediaType>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct MetadataEditorInfo {
    pub parental_rating_options: Vec<ParentalRating>,
    pub countries: Vec<CountryInfo>,
    pub cultures: Vec<CultureDto>,
    pub external_id_infos: Vec<ExternalIdInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<CollectionType>,
    pub content_type_options: Vec<NameValuePair>,
}
