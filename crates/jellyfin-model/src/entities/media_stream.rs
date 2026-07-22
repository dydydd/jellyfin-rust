use serde::{Deserialize, Serialize};

const SPECIAL_LANGUAGE_CODES: &[&str] = &["mis", "mul", "und", "zxx"];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum MediaStreamType {
    #[default]
    Audio = 0,
    Video = 1,
    Subtitle = 2,
    EmbeddedImage = 3,
    Data = 4,
    Lyric = 5,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum VideoRange {
    #[default]
    Unknown = 0,
    #[serde(rename = "SDR")]
    Sdr = 1,
    #[serde(rename = "HDR")]
    Hdr = 2,
}

impl VideoRange {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Sdr => "SDR",
            Self::Hdr => "HDR",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum VideoRangeType {
    #[default]
    Unknown = 0,
    #[serde(rename = "SDR")]
    Sdr = 1,
    #[serde(rename = "HDR10")]
    Hdr10 = 2,
    #[serde(rename = "HLG")]
    Hlg = 3,
    #[serde(rename = "DOVI")]
    Dovi = 4,
    #[serde(rename = "DOVIWithHDR10")]
    DoviWithHdr10 = 5,
    #[serde(rename = "DOVIWithHLG")]
    DoviWithHlg = 6,
    #[serde(rename = "DOVIWithSDR")]
    DoviWithSdr = 7,
    #[serde(rename = "DOVIWithEL")]
    DoviWithEl = 8,
    #[serde(rename = "DOVIWithHDR10Plus")]
    DoviWithHdr10Plus = 9,
    #[serde(rename = "DOVIWithELHDR10Plus")]
    DoviWithElHdr10Plus = 10,
    #[serde(rename = "DOVIInvalid")]
    DoviInvalid = 11,
    #[serde(rename = "HDR10Plus")]
    Hdr10Plus = 12,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum AudioSpatialFormat {
    #[default]
    None = 0,
    DolbyAtmos = 1,
    #[serde(rename = "DTSX")]
    DtsX = 2,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum SubtitleDeliveryMethod {
    #[default]
    Encode = 0,
    Embed = 1,
    External = 2,
    Hls = 3,
    Drop = 4,
}

/// API-facing media stream plus Jellyfin's pure display calculations.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MediaStream {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_transfer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dv_profile: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpu_present_flag: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bl_present_flag: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dv_bl_signal_compatibility_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdr10_plus_present_flag: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_undefined: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_forced: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_external: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_hearing_impaired: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_original: Option<String>,
    pub is_interlaced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    pub index: i32,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
    pub is_original: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_frame_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_frame_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(rename = "Type")]
    pub stream_type: MediaStreamType,
    pub is_external: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_method: Option<SubtitleDeliveryMethod>,
}

impl MediaStream {
    #[must_use]
    pub fn display_title(&self) -> Option<String> {
        match self.stream_type {
            MediaStreamType::Audio => Some(self.audio_display_title()),
            MediaStreamType::Video => Some(self.video_display_title()),
            MediaStreamType::Subtitle => Some(self.subtitle_display_title()),
            MediaStreamType::EmbeddedImage | MediaStreamType::Data | MediaStreamType::Lyric => None,
        }
    }

    #[must_use]
    pub fn get_resolution_text(&self) -> Option<&'static str> {
        let (Some(width), Some(height)) = (self.width, self.height) else {
            return None;
        };
        let interlaced = self.is_interlaced;

        match width {
            ..=256 if height <= 144 => Some(if interlaced { "144i" } else { "144p" }),
            ..=426 if height <= 240 => Some(if interlaced { "240i" } else { "240p" }),
            ..=640 if height <= 360 => Some(if interlaced { "360i" } else { "360p" }),
            ..=682 if height <= 384 => Some(if interlaced { "384i" } else { "384p" }),
            ..=720 if height <= 404 => Some(if interlaced { "404i" } else { "404p" }),
            ..=854 if height <= 480 => Some(if interlaced { "480i" } else { "480p" }),
            ..=960 if height <= 544 => Some(if interlaced { "540i" } else { "540p" }),
            ..=1024 if height <= 576 => Some(if interlaced { "576i" } else { "576p" }),
            ..=1280 if height <= 962 => Some(if interlaced { "720i" } else { "720p" }),
            ..=2560 if height <= 1440 => Some(if interlaced { "1080i" } else { "1080p" }),
            ..=4096 if height <= 3072 => Some("4K"),
            ..=8192 if height <= 6144 => Some("8K"),
            _ => None,
        }
    }

    #[must_use]
    pub fn reference_frame_rate(&self) -> Option<f32> {
        match self.average_frame_rate {
            Some(rate) if rate < 1000.0 => Some(rate),
            _ => self.real_frame_rate,
        }
    }

    #[must_use]
    pub fn is_text_subtitle_stream(&self) -> bool {
        if self.stream_type != MediaStreamType::Subtitle {
            return false;
        }
        if non_empty(self.codec.as_deref()).is_none() && !self.is_external {
            return false;
        }
        Self::is_text_format(self.codec.as_deref())
    }

    #[must_use]
    pub fn is_pgs_subtitle_stream(&self) -> bool {
        if self.stream_type != MediaStreamType::Subtitle {
            return false;
        }
        if non_empty(self.codec.as_deref()).is_none() && !self.is_external {
            return false;
        }
        Self::is_pgs_format(self.codec.as_deref())
    }

    #[must_use]
    pub fn is_vobsub_subtitle_stream(&self) -> bool {
        if self.stream_type != MediaStreamType::Subtitle {
            return false;
        }
        if non_empty(self.codec.as_deref()).is_none() && !self.is_external {
            return false;
        }
        Self::is_vobsub_format(self.codec.as_deref())
    }

    #[must_use]
    pub fn is_extractable_subtitle_stream(&self) -> bool {
        self.is_text_subtitle_stream()
            || self.is_pgs_subtitle_stream()
            || self.is_vobsub_subtitle_stream()
    }

    #[must_use]
    pub fn is_text_format(format: Option<&str>) -> bool {
        let codec = format.unwrap_or_default();
        contains_ignore_case(codec, "microdvd")
            || (!contains_ignore_case(codec, "pgs")
                && !contains_ignore_case(codec, "dvdsub")
                && !contains_ignore_case(codec, "vobsub")
                && !contains_ignore_case(codec, "dvbsub")
                && !codec.eq_ignore_ascii_case("sup")
                && !codec.eq_ignore_ascii_case("sub"))
    }

    #[must_use]
    pub fn is_pgs_format(format: Option<&str>) -> bool {
        let codec = format.unwrap_or_default();
        contains_ignore_case(codec, "pgs") || codec.eq_ignore_ascii_case("sup")
    }

    #[must_use]
    pub fn is_vobsub_format(format: Option<&str>) -> bool {
        let codec = format.unwrap_or_default();
        contains_ignore_case(codec, "dvdsub") || contains_ignore_case(codec, "vobsub")
    }

    #[must_use]
    pub fn supports_subtitle_conversion_to(&self, target_codec: &str) -> bool {
        if !self.is_text_subtitle_stream() {
            return false;
        }
        let source_codec = self.codec.as_deref().unwrap_or_default();
        !source_codec.eq_ignore_ascii_case("ass")
            && !source_codec.eq_ignore_ascii_case("ssa")
            && !target_codec.eq_ignore_ascii_case("ass")
            && !target_codec.eq_ignore_ascii_case("ssa")
    }

    #[must_use]
    pub fn audio_spatial_format(&self) -> AudioSpatialFormat {
        if self.stream_type != MediaStreamType::Audio {
            return AudioSpatialFormat::None;
        }
        let Some(profile) = self
            .profile
            .as_deref()
            .filter(|profile| !profile.is_empty())
        else {
            return AudioSpatialFormat::None;
        };
        if contains_ignore_case(profile, "Dolby Atmos") {
            AudioSpatialFormat::DolbyAtmos
        } else if contains_ignore_case(profile, "DTS:X") {
            AudioSpatialFormat::DtsX
        } else {
            AudioSpatialFormat::None
        }
    }

    #[must_use]
    pub fn video_dovi_title(&self) -> Option<String> {
        let profile = self.dv_profile?;
        if self.rpu_present_flag != Some(1)
            || self.bl_present_flag != Some(1)
            || !matches!(profile, 4 | 5 | 7 | 8 | 9 | 10)
        {
            return None;
        }

        let compatibility = self.dv_bl_signal_compatibility_id.unwrap_or_default();
        let mut title = format!("Dolby Vision Profile {profile}");
        if compatibility > 0 {
            title.push_str(&format!(".{compatibility}"));
        }
        match compatibility {
            1 | 6 => title.push_str(" (HDR10)"),
            2 => title.push_str(" (SDR)"),
            4 => title.push_str(" (HLG)"),
            _ => {}
        }
        Some(title)
    }

    #[must_use]
    pub fn video_range(&self) -> VideoRange {
        self.get_video_color_range().0
    }

    #[must_use]
    pub fn video_range_type(&self) -> VideoRangeType {
        self.get_video_color_range().1
    }

    #[must_use]
    pub fn get_video_color_range(&self) -> (VideoRange, VideoRangeType) {
        if self.stream_type != MediaStreamType::Video {
            return (VideoRange::Unknown, VideoRangeType::Unknown);
        }

        let profile = self.dv_profile;
        let compatibility = self.dv_bl_signal_compatibility_id;
        let dovi_profile = matches!(profile, Some(5 | 7 | 8 | 10));
        let dovi_flags = self.rpu_present_flag == Some(1)
            && self.bl_present_flag == Some(1)
            && matches!(compatibility, Some(0 | 1 | 2 | 4 | 6));
        let dovi_tag = self.codec_tag.as_deref().is_some_and(|tag| {
            ["dovi", "dvh1", "dvhe", "dav1"]
                .iter()
                .any(|candidate| tag.eq_ignore_ascii_case(candidate))
        });

        if (dovi_profile && dovi_flags) || dovi_tag {
            let mut range = match profile {
                Some(5) => (VideoRange::Hdr, VideoRangeType::Dovi),
                Some(8) => match compatibility {
                    Some(1) => (VideoRange::Hdr, VideoRangeType::DoviWithHdr10),
                    Some(4) => (VideoRange::Hdr, VideoRangeType::DoviWithHlg),
                    Some(2) => (VideoRange::Sdr, VideoRangeType::DoviWithSdr),
                    _ => (VideoRange::Hdr, VideoRangeType::DoviInvalid),
                },
                Some(7) => (VideoRange::Hdr, VideoRangeType::DoviWithEl),
                Some(10) => match compatibility {
                    Some(0) => (VideoRange::Hdr, VideoRangeType::Dovi),
                    Some(1) => (VideoRange::Hdr, VideoRangeType::DoviWithHdr10),
                    Some(2) => (VideoRange::Sdr, VideoRangeType::DoviWithSdr),
                    Some(4) => (VideoRange::Hdr, VideoRangeType::DoviWithHlg),
                    _ => (VideoRange::Hdr, VideoRangeType::DoviInvalid),
                },
                _ => (VideoRange::Sdr, VideoRangeType::Sdr),
            };
            if self.hdr10_plus_present_flag == Some(true) {
                range.1 = match range.1 {
                    VideoRangeType::DoviWithHdr10 => VideoRangeType::DoviWithHdr10Plus,
                    VideoRangeType::DoviWithEl => VideoRangeType::DoviWithElHdr10Plus,
                    value => value,
                };
            }
            return range;
        }

        match self.color_transfer.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("smpte2084") => {
                if self.hdr10_plus_present_flag == Some(true) {
                    (VideoRange::Hdr, VideoRangeType::Hdr10Plus)
                } else {
                    (VideoRange::Hdr, VideoRangeType::Hdr10)
                }
            }
            Some(value) if value.eq_ignore_ascii_case("arib-std-b67") => {
                (VideoRange::Hdr, VideoRangeType::Hlg)
            }
            _ => (VideoRange::Sdr, VideoRangeType::Sdr),
        }
    }

    fn audio_display_title(&self) -> String {
        let mut attributes = Vec::new();
        if let Some(language) = non_empty(self.language.as_deref())
            && !SPECIAL_LANGUAGE_CODES
                .iter()
                .any(|code| language.eq_ignore_ascii_case(code))
        {
            attributes.push(first_to_upper(
                self.localized_language.as_deref().unwrap_or(language),
            ));
        }

        if let Some(profile) = non_empty(self.profile.as_deref())
            && !profile.eq_ignore_ascii_case("lc")
        {
            attributes.push(profile.to_owned());
        } else if let Some(codec) = non_empty(self.codec.as_deref()) {
            attributes.push(friendly_audio_codec(codec));
        }

        if let Some(layout) = non_empty(self.channel_layout.as_deref()) {
            attributes.push(first_to_upper(layout));
        } else if let Some(channels) = self.channels {
            attributes.push(format!("{channels} ch"));
        }
        if self.is_default {
            attributes.push(localized_or(&self.localized_default, "Default"));
        }
        if self.is_external {
            attributes.push(localized_or(&self.localized_external, "External"));
        }
        if self.is_original {
            attributes.push(localized_or(&self.localized_original, "Original"));
        }
        with_title(self.title.as_deref(), &attributes, " - ")
    }

    fn video_display_title(&self) -> String {
        let mut attributes = Vec::new();
        if let Some(resolution) = self.get_resolution_text() {
            attributes.push(resolution.to_owned());
        }
        if let Some(codec) = non_empty(self.codec.as_deref()) {
            attributes.push(codec.to_uppercase());
        }
        if let Some(dovi) = self.video_dovi_title() {
            attributes.push(dovi);
        } else {
            let range = self.video_range();
            if range != VideoRange::Unknown {
                attributes.push(range.as_str().to_owned());
            }
        }
        with_title(self.title.as_deref(), &attributes, " ")
    }

    fn subtitle_display_title(&self) -> String {
        let mut attributes = Vec::new();
        if let Some(language) = non_empty(self.language.as_deref()) {
            attributes.push(first_to_upper(
                self.localized_language.as_deref().unwrap_or(language),
            ));
        } else {
            attributes.push(localized_or(&self.localized_undefined, "Und"));
        }
        if self.is_hearing_impaired {
            attributes.push(localized_or(
                &self.localized_hearing_impaired,
                "Hearing Impaired",
            ));
        }
        if self.is_default {
            attributes.push(localized_or(&self.localized_default, "Default"));
        }
        if self.is_forced {
            attributes.push(localized_or(&self.localized_forced, "Forced"));
        }
        if let Some(codec) = non_empty(self.codec.as_deref()) {
            attributes.push(codec.to_uppercase());
        }
        if self.is_external {
            attributes.push(localized_or(&self.localized_external, "External"));
        }
        with_title(self.title.as_deref(), &attributes, " - ")
    }
}

fn with_title(title: Option<&str>, attributes: &[String], separator: &str) -> String {
    let Some(title) = non_empty(title) else {
        return attributes.join(separator);
    };
    let mut result = title.to_owned();
    for attribute in attributes {
        if !contains_ignore_case(title, attribute) {
            result.push_str(" - ");
            result.push_str(attribute);
        }
    }
    result
}

fn friendly_audio_codec(codec: &str) -> String {
    if codec.eq_ignore_ascii_case("ac3") {
        "Dolby Digital".to_owned()
    } else if codec.eq_ignore_ascii_case("eac3") {
        "Dolby Digital+".to_owned()
    } else if codec.eq_ignore_ascii_case("dca") {
        "DTS".to_owned()
    } else {
        codec.to_uppercase()
    }
}

fn first_to_upper(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    if !first.is_lowercase() {
        return value.to_owned();
    }
    first.to_uppercase().chain(chars).collect()
}

fn localized_or(value: &Option<String>, fallback: &str) -> String {
    non_empty(value.as_deref()).unwrap_or(fallback).to_owned()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}
