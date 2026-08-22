use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::ProviderIdMap;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    #[default]
    TV,
    Radio,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgramAudio {
    #[default]
    Mono,
    Stereo,
    Dolby,
    DolbyDigital,
    Thx,
    Atmos,
}

/// Internal timer data shared by Live TV recording services.
///
/// This is the subset of Jellyfin's controller `TimerInfo` contract used to
/// derive recording names and persist recording NFO metadata. Additional
/// timer fields can be added here as their owning Live TV features are ported.
#[derive(Clone, Debug, PartialEq)]
pub struct TimerInfo {
    pub name: String,
    pub program_id: Option<String>,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    pub community_rating: Option<f32>,
    pub official_rating: Option<String>,
    pub provider_ids: ProviderIdMap,
    pub series_provider_ids: ProviderIdMap,
    pub start_date: DateTime<Utc>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub is_movie: bool,
    pub production_year: Option<i32>,
    pub episode_title: Option<String>,
    pub original_air_date: Option<DateTime<Utc>>,
    pub is_program_series: bool,
    pub is_repeat: bool,
    pub is_sports: bool,
    pub is_kids: bool,
    pub is_news: bool,
}

/// Configuration and discovered identity for a Live TV tuner host.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "PascalCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct TunerHostInfo {
    pub id: Option<String>,
    pub url: String,
    #[serde(rename = "Type")]
    pub tuner_type: String,
    pub device_id: Option<String>,
    pub friendly_name: Option<String>,
    pub import_favorites_only: bool,
    #[serde(rename = "AllowHWTranscoding")]
    pub allow_hw_transcoding: bool,
    pub allow_fmp4_transcoding_container: bool,
    pub allow_stream_sharing: bool,
    pub fallback_max_streaming_bitrate: i32,
    pub enable_stream_looping: bool,
    pub source: Option<String>,
    pub tuner_count: i32,
    pub user_agent: Option<String>,
    pub ignore_dts: bool,
    pub read_at_native_framerate: bool,
}

impl Default for TunerHostInfo {
    fn default() -> Self {
        Self {
            id: None,
            url: String::new(),
            tuner_type: String::new(),
            device_id: None,
            friendly_name: None,
            import_favorites_only: false,
            allow_hw_transcoding: true,
            allow_fmp4_transcoding_container: false,
            allow_stream_sharing: true,
            fallback_max_streaming_bitrate: 30_000_000,
            enable_stream_looping: false,
            source: None,
            tuner_count: 0,
            user_agent: None,
            ignore_dts: true,
            read_at_native_framerate: false,
        }
    }
}

impl Default for TimerInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            program_id: None,
            overview: None,
            genres: Vec::new(),
            community_rating: None,
            official_rating: None,
            provider_ids: ProviderIdMap::new(),
            series_provider_ids: ProviderIdMap::new(),
            start_date: dotnet_min_utc(),
            season_number: None,
            episode_number: None,
            is_movie: false,
            production_year: None,
            episode_title: None,
            original_air_date: None,
            is_program_series: false,
            is_repeat: false,
            is_sports: false,
            is_kids: false,
            is_news: false,
        }
    }
}

fn dotnet_min_utc() -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(1, 1, 1)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| DateTime::from_naive_utc_and_offset(date, Utc))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}
