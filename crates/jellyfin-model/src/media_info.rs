use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeSeq};
use uuid::Uuid;

use crate::{BaseItemPerson, ChapterInfo, DeviceProfile, MediaSourceInfo, MediaStream};

pub use crate::dlna::{MediaProtocol, PlaybackErrorCode, TransportStreamTimestamp};

/// Friendly audio codec names used by Jellyfin's media info helpers.
pub struct AudioCodec;

impl AudioCodec {
    #[must_use]
    pub fn get_friendly_name(codec: &str) -> String {
        if codec.is_empty() {
            return codec.to_owned();
        }

        match codec.to_ascii_lowercase().as_str() {
            "ac3" => "Dolby Digital".to_owned(),
            "eac3" => "Dolby Digital+".to_owned(),
            "dca" => "DTS".to_owned(),
            _ => codec.to_uppercase(),
        }
    }
}

/// How the default audio index is determined.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AudioIndexSource(u8);

impl AudioIndexSource {
    pub const NONE: Self = Self(0);
    pub const DEFAULT: Self = Self(1 << 0);
    pub const LANGUAGE: Self = Self(1 << 1);
    pub const USER: Self = Self(1 << 2);

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl std::ops::BitOr for AudioIndexSource {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for AudioIndexSource {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl Serialize for AudioIndexSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(None)?;
        if self.is_empty() {
            sequence.serialize_element("None")?;
        } else {
            if self.contains(Self::DEFAULT) {
                sequence.serialize_element("Default")?;
            }
            if self.contains(Self::LANGUAGE) {
                sequence.serialize_element("Language")?;
            }
            if self.contains(Self::USER) {
                sequence.serialize_element("User")?;
            }
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for AudioIndexSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<String>::deserialize(deserializer)?;
        let mut result = Self::NONE;
        for value in values {
            match value.as_str() {
                "None" => {}
                "Default" => result.insert(Self::DEFAULT),
                "Language" => result.insert(Self::LANGUAGE),
                "User" => result.insert(Self::USER),
                _ => {
                    return Err(D::Error::unknown_variant(
                        &value,
                        &["None", "Default", "Language", "User"],
                    ));
                }
            }
        }
        Ok(result)
    }
}

/// Represents the result of BDInfo output.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BlurayDiscInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_streams: Option<Vec<MediaStream>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapters: Option<Vec<f64>>,
}

/// Inspects Blu-ray disc structure.
pub trait BlurayExaminer {
    fn get_disc_info(&self, path: &str) -> BlurayDiscInfo;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LiveStreamRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_token: Option<String>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub user_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_streaming_bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_audio_channels: Option<i32>,
    #[serde(default, with = "crate::serde_guid::single")]
    pub item_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_profile: Option<DeviceProfile>,
    pub enable_direct_play: bool,
    pub enable_direct_stream: bool,
    pub always_burn_in_subtitle_when_transcoding: bool,
    pub direct_play_protocols: Vec<MediaProtocol>,
}

impl Default for LiveStreamRequest {
    fn default() -> Self {
        Self {
            open_token: None,
            user_id: Uuid::nil(),
            play_session_id: None,
            max_streaming_bitrate: None,
            start_time_ticks: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
            max_audio_channels: None,
            item_id: Uuid::nil(),
            device_profile: None,
            enable_direct_play: true,
            enable_direct_stream: true,
            always_burn_in_subtitle_when_transcoding: false,
            direct_play_protocols: vec![MediaProtocol::Http],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LiveStreamResponse {
    pub media_source: MediaSourceInfo,
}

/// Extended media source metadata produced by media probing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MediaInfo {
    #[serde(flatten)]
    pub media_source: MediaSourceInfo,
    pub chapters: Vec<ChapterInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    pub artists: Vec<String>,
    pub album_artists: Vec<String>,
    pub studios: Vec<String>,
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forced_sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub premiere_date: Option<DateTime<Utc>>,
    pub people: Vec<BaseItemPerson>,
    pub provider_ids: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_rating_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
}

impl Default for MediaInfo {
    fn default() -> Self {
        Self {
            media_source: MediaSourceInfo::default(),
            chapters: Vec::new(),
            album: None,
            artists: Vec::new(),
            album_artists: Vec::new(),
            studios: Vec::new(),
            genres: Vec::new(),
            show_name: None,
            forced_sort_name: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            people: Vec::new(),
            provider_ids: HashMap::new(),
            official_rating: None,
            official_rating_description: None,
            overview: None,
        }
    }
}

/// Response returned by the playback-info endpoint.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    pub media_sources: Vec<MediaSourceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<PlaybackErrorCode>,
}

/// Subtitle format constants used by the media info layer.
pub struct SubtitleFormat;

impl SubtitleFormat {
    pub const SRT: &'static str = "srt";
    pub const SUBRIP: &'static str = "subrip";
    pub const SSA: &'static str = "ssa";
    pub const ASS: &'static str = "ass";
    pub const VTT: &'static str = "vtt";
    pub const WEBVTT: &'static str = "webvtt";
    pub const TTML: &'static str = "ttml";
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SubtitleTrackEvent {
    pub id: String,
    pub text: String,
    pub start_position_ticks: i64,
    pub end_position_ticks: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SubtitleTrackInfo {
    pub track_events: Vec<SubtitleTrackEvent>,
}
