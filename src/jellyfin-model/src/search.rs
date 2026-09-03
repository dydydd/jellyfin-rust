use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::MediaType;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct SearchHint {
    #[cfg_attr(feature = "openapi", schemars(with = "String"))]
    #[serde(with = "crate::serde_guid::single")]
    pub item_id: Uuid,
    #[cfg_attr(feature = "openapi", schemars(with = "String"))]
    #[serde(with = "crate::serde_guid::single")]
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_term: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_image_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_image_item_id: Option<String>,
    #[serde(rename = "Type")]
    pub item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_folder: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    pub media_type: MediaType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[cfg_attr(feature = "openapi", schemars(with = "Option<String>"))]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::serde_guid::option::serialize"
    )]
    pub album_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    pub artists: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<i32>,
    #[cfg_attr(feature = "openapi", schemars(with = "Option<String>"))]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::serde_guid::option::serialize"
    )]
    pub channel_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,
}

impl Default for SearchHint {
    fn default() -> Self {
        Self {
            item_id: Uuid::nil(),
            id: Uuid::nil(),
            name: String::new(),
            matched_term: Some(String::new()),
            index_number: None,
            production_year: None,
            parent_index_number: None,
            primary_image_tag: None,
            thumb_image_tag: None,
            thumb_image_item_id: None,
            backdrop_image_tag: None,
            backdrop_image_item_id: None,
            item_type: String::new(),
            is_folder: None,
            run_time_ticks: None,
            media_type: MediaType::Unknown,
            start_date: None,
            end_date: None,
            series: None,
            status: None,
            album: None,
            album_id: None,
            album_artist: None,
            artists: Vec::new(),
            song_count: None,
            episode_count: None,
            channel_id: None,
            channel_name: None,
            primary_image_aspect_ratio: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct SearchHintResult {
    pub search_hints: Vec<SearchHint>,
    pub total_record_count: usize,
}
