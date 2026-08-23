use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Type of content represented by a media segment.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum MediaSegmentType {
    #[default]
    Unknown = 0,
    Commercial = 1,
    Preview = 2,
    Recap = 3,
    Outro = 4,
    Intro = 5,
}

impl std::str::FromStr for MediaSegmentType {
    type Err = ParseMediaSegmentTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("Unknown") {
            Ok(Self::Unknown)
        } else if value.eq_ignore_ascii_case("Commercial") {
            Ok(Self::Commercial)
        } else if value.eq_ignore_ascii_case("Preview") {
            Ok(Self::Preview)
        } else if value.eq_ignore_ascii_case("Recap") {
            Ok(Self::Recap)
        } else if value.eq_ignore_ascii_case("Outro") {
            Ok(Self::Outro)
        } else if value.eq_ignore_ascii_case("Intro") {
            Ok(Self::Intro)
        } else {
            Err(ParseMediaSegmentTypeError)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseMediaSegmentTypeError;

impl fmt::Display for ParseMediaSegmentTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid media segment type")
    }
}

impl Error for ParseMediaSegmentTypeError {}

/// API model for Jellyfin media segments.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MediaSegmentDto {
    #[serde(with = "crate::serde_guid::single")]
    pub id: Uuid,
    #[serde(with = "crate::serde_guid::single")]
    pub item_id: Uuid,
    #[serde(rename = "Type")]
    pub segment_type: MediaSegmentType,
    pub start_ticks: i64,
    pub end_ticks: i64,
}
