use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BaseItemDto, BaseItemKind, ImageType, NameGuidPair, NameValuePair};

/// Controls which optional fields are attached to a `BaseItemDto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ItemFields {
    AirTime,
    CanDelete,
    CanDownload,
    ChannelInfo,
    Chapters,
    Trickplay,
    ChildCount,
    CumulativeRunTimeTicks,
    CustomRating,
    DateCreated,
    DateLastMediaAdded,
    DisplayPreferencesId,
    Etag,
    ExternalUrls,
    Genres,
    ItemCounts,
    MediaSourceCount,
    MediaSources,
    OriginalTitle,
    Overview,
    ParentId,
    Path,
    People,
    PlayAccess,
    ProductionLocations,
    ProviderIds,
    PrimaryImageAspectRatio,
    RecursiveItemCount,
    Settings,
    SeriesStudio,
    SortName,
    SpecialEpisodeNumbers,
    Studios,
    Taglines,
    Tags,
    RemoteTrailers,
    MediaStreams,
    SeasonUserData,
    DateLastRefreshed,
    DateLastSaved,
    RefreshState,
    ChannelImage,
    EnableMediaSourceDisplay,
    Width,
    Height,
    ExtraIds,
    LocalTrailerCount,
    IsHD,
    SpecialFeatureCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum ItemFilter {
    IsFolder = 1,
    IsNotFolder = 2,
    IsUnplayed = 3,
    IsPlayed = 4,
    IsFavorite = 5,
    IsResumable = 7,
    Likes = 8,
    Dislikes = 9,
    IsFavoriteOrLikes = 10,
}

/// Generic paged item container used by most list endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult<T> {
    pub items: Vec<T>,
    pub total_record_count: usize,
    pub start_index: usize,
}

impl<T> Default for QueryResult<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total_record_count: 0,
            start_index: 0,
        }
    }
}

impl<T> QueryResult<T> {
    #[must_use]
    pub fn from_items(items: Vec<T>) -> Self {
        Self {
            total_record_count: items.len(),
            items,
            start_index: 0,
        }
    }

    #[must_use]
    pub fn paged(start_index: usize, total_record_count: usize, items: Vec<T>) -> Self {
        Self {
            items,
            total_record_count,
            start_index,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LatestItemsQuery {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub user_id: Option<Uuid>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub parent_id: Uuid,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub fields: Vec<ItemFields>,
    pub include_item_types: Vec<BaseItemKind>,
    pub is_played: Option<bool>,
    pub group_items: bool,
    pub enable_images: Option<bool>,
    pub image_type_limit: Option<i32>,
    pub enable_image_types: Vec<ImageType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct NextUpQuery {
    #[serde(default, with = "crate::serde_guid::single")]
    pub user_id: Uuid,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub parent_id: Option<Uuid>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub series_id: Option<Uuid>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub enable_image_types: Vec<ImageType>,
    #[serde(default = "default_true")]
    pub enable_total_record_count: bool,
    #[serde(with = "crate::serde_datetime::required")]
    pub next_up_date_cutoff: DateTime<Utc>,
    pub enable_resumable: bool,
    pub enable_rewatching: bool,
}

impl Default for NextUpQuery {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            parent_id: None,
            series_id: None,
            start_index: None,
            limit: None,
            enable_image_types: Vec::new(),
            enable_total_record_count: true,
            next_up_date_cutoff: DateTime::<Utc>::UNIX_EPOCH,
            enable_resumable: false,
            enable_rewatching: false,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct QueryFilters {
    pub genres: Vec<NameGuidPair>,
    pub tags: Vec<String>,
    pub audio_languages: Vec<NameValuePair>,
    pub subtitle_languages: Vec<NameValuePair>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct QueryFiltersLegacy {
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub official_ratings: Vec<String>,
    pub years: Vec<i32>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ThemeMediaResult {
    #[serde(flatten)]
    pub query_result: QueryResult<BaseItemDto>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub owner_id: Uuid,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AllThemeMediaResult {
    pub theme_videos_result: ThemeMediaResult,
    pub theme_songs_result: ThemeMediaResult,
    pub soundtrack_songs_result: ThemeMediaResult,
}

const fn default_true() -> bool {
    true
}
