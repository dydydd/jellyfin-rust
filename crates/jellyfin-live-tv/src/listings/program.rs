use std::fmt;

use chrono::{DateTime, Utc};
use jellyfin_model::ProviderIdMap;

/// Audio presentation advertised for a guide programme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramAudio {
    Mono,
    Stereo,
    Dolby,
    DolbyDigital,
    Thx,
    Atmos,
}

impl fmt::Display for ProgramAudio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Mono => "Mono",
            Self::Stereo => "Stereo",
            Self::Dolby => "Dolby",
            Self::DolbyDigital => "DolbyDigital",
            Self::Thx => "Thx",
            Self::Atmos => "Atmos",
        })
    }
}

/// Boolean programme attributes stored as a compact set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum ProgramFlag {
    Repeat = 1 << 0,
    SubjectToBlackout = 1 << 1,
    Movie = 1 << 2,
    Sports = 1 << 3,
    Series = 1 << 4,
    Live = 1 << 5,
    News = 1 << 6,
    Kids = 1 << 7,
    Educational = 1 << 8,
    Premiere = 1 << 9,
}

/// Set of boolean programme attributes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProgramFlags(u16);

impl ProgramFlags {
    #[must_use]
    pub const fn contains(self, flag: ProgramFlag) -> bool {
        self.0 & flag as u16 != 0
    }

    pub const fn insert(&mut self, flag: ProgramFlag) {
        self.0 |= flag as u16;
    }

    pub const fn remove(&mut self, flag: ProgramFlag) {
        self.0 &= !(flag as u16);
    }

    pub const fn set(&mut self, flag: ProgramFlag, enabled: bool) {
        if enabled {
            self.insert(flag);
        } else {
            self.remove(flag);
        }
    }
}

/// Provider-independent guide programme consumed by Jellyfin's guide layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgramInfo {
    pub id: Option<String>,
    pub channel_id: Option<String>,
    pub name: Option<String>,
    pub official_rating: Option<String>,
    pub overview: Option<String>,
    pub short_overview: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub genres: Vec<String>,
    pub original_air_date: Option<DateTime<Utc>>,
    pub is_hd: Option<bool>,
    pub is_3d: Option<bool>,
    pub audio: Option<ProgramAudio>,
    pub community_rating: Option<f32>,
    pub flags: ProgramFlags,
    pub episode_title: Option<String>,
    pub image_path: Option<String>,
    pub image_url: Option<String>,
    pub thumb_image_url: Option<String>,
    pub logo_image_url: Option<String>,
    pub backdrop_image_url: Option<String>,
    pub has_image: Option<bool>,
    pub production_year: Option<i32>,
    pub home_page_url: Option<String>,
    pub series_id: Option<String>,
    pub show_id: Option<String>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub etag: Option<String>,
    pub provider_ids: ProviderIdMap,
    pub series_provider_ids: ProviderIdMap,
}
