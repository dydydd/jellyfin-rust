use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    BaseItemKind, ChannelType, ChapterInfo, CollectionType, DayOfWeek, ExternalUrl, ExtraType,
    ImageOrientation, ImageType, IsoType, LocationType, MediaSourceInfo, MediaStream, MediaType,
    MetadataField, PersonKind, PlayAccess, ProgramAudio, UserConfiguration, UserItemDataDto,
    UserPolicy, Video3DFormat, VideoType,
};

/// Public metadata for one trickplay thumbnail resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct TrickplayInfoDto {
    pub width: i32,
    pub height: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub thumbnail_count: i32,
    pub interval: i32,
    pub bandwidth: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct NameIdPair {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct NameGuidPair {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub id: Uuid,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BaseItemPerson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    #[serde(rename = "Type")]
    pub person_type: PersonKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_blur_hashes: Option<HashMap<ImageType, HashMap<String, String>>>,
}

/// Data transfer object for a library item, matching the official API contract.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BaseItemDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_item_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub date_created: Option<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub date_last_media_added: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_type: Option<ExtraType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airs_before_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airs_after_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airs_before_episode_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_delete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_download: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_lyrics: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_subtitles: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_metadata_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_metadata_country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forced_sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_3d_format: Option<Video3DFormat>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub premiere_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_urls: Option<Vec<ExternalUrl>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_sources: Option<Vec<MediaSourceInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critic_rating: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_locations: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_media_source_display: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_rating: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub channel_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taglines: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_run_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_access: Option<PlayAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_place_holder: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number_end: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_trailers: Option<Vec<crate::MediaUrl>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_hd: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_folder: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    #[serde(rename = "Type")]
    pub item_type: BaseItemKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub people: Option<Vec<BaseItemPerson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studios: Option<Vec<NameGuidPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre_items: Option<Vec<NameGuidPair>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_logo_item_id: Option<Uuid>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_backdrop_item_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_backdrop_image_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_trailer_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<UserItemDataDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recursive_item_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub series_id: Option<Uuid>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub season_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub special_feature_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_preferences_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_days: Option<Vec<DayOfWeek>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artists: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_items: Option<Vec<NameGuidPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_type: Option<CollectionType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_order: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub album_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artists: Option<Vec<NameGuidPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_streams: Option<Vec<MediaStream>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_type: Option<VideoType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_source_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tags: Option<HashMap<ImageType, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_image_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_image_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_logo_image_tag: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_art_item_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_art_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_thumb_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_blur_hashes: Option<HashMap<ImageType, HashMap<String, String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_studio: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_thumb_item_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_thumb_image_tag: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_primary_image_item_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapters: Option<Vec<ChapterInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trickplay: Option<HashMap<String, HashMap<i32, TrickplayInfoDto>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_type: Option<LocationType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso_type: Option<IsoType>,
    #[serde(default)]
    pub media_type: MediaType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub end_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_fields: Option<Vec<MetadataField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailer_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_video_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_data: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_make: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_orientation: Option<ImageOrientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutter_speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub altitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso_speed_rating: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_timer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_primary_image_tag: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub start_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_percentage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_repeat: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<ChannelType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<ProgramAudio>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_movie: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_sports: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_series: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_live: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_news: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_kids: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_premiere: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalization_gain: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_normalization_gain: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_program: Option<Box<BaseItemDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UserDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(with = "crate::serde_guid::single")]
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_configured_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_configured_easy_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_auto_login: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub last_login_date: Option<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub last_activity_date: Option<DateTime<Utc>>,
    pub configuration: UserConfiguration,
    pub policy: UserPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,
}

impl Default for UserDto {
    fn default() -> Self {
        Self {
            name: None,
            server_id: None,
            server_name: None,
            id: Uuid::nil(),
            primary_image_tag: None,
            has_password: Some(true),
            has_configured_password: Some(true),
            has_configured_easy_password: Some(false),
            enable_auto_login: None,
            last_login_date: None,
            last_activity_date: None,
            configuration: UserConfiguration::default(),
            policy: UserPolicy::default(),
            primary_image_aspect_ratio: None,
        }
    }
}

impl std::fmt::Display for UserDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => formatter.write_str(name),
            None => formatter.write_str(&self.id.simple().to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct ItemCounts {
    pub movie_count: i32,
    pub series_count: i32,
    pub episode_count: i32,
    pub artist_count: i32,
    pub program_count: i32,
    pub trailer_count: i32,
    pub song_count: i32,
    pub album_count: i32,
    pub music_video_count: i32,
    pub box_set_count: i32,
    pub book_count: i32,
    pub item_count: i32,
}
