use std::{collections::HashMap, fmt::Write};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{MediaAttachment, SubtitleDeliveryMethod};

mod stream_builder;

pub use stream_builder::{
    CodecProfile, CodecType, DeviceProfile, DirectPlayProfile, MediaOptions, StreamBuilder,
    StreamBuilderError, SubtitleProfile, TranscodingProfile,
};

/// Helpers for matching comma-delimited media container lists.
pub struct ContainerHelper;

impl ContainerHelper {
    /// Matches the nullable string overload of Jellyfin's `ContainsContainer`.
    #[must_use]
    pub fn contains_container(
        profile_containers: Option<&str>,
        input_container: Option<&str>,
    ) -> bool {
        let (profile_containers, is_negative_list) = split_negative_list(profile_containers);
        Self::contains_container_with_polarity(
            profile_containers,
            is_negative_list,
            input_container,
        )
    }

    /// Matches the span overload of Jellyfin's `ContainsContainer`.
    #[must_use]
    pub fn contains_container_span(
        profile_containers: Option<&str>,
        input_container: &str,
    ) -> bool {
        let (profile_containers, is_negative_list) = split_negative_list(profile_containers);
        Self::contains_container_span_with_polarity(
            profile_containers,
            is_negative_list,
            input_container,
        )
    }

    /// Matches the nullable string overload with an explicit list polarity.
    #[must_use]
    pub fn contains_container_with_polarity(
        profile_containers: Option<&str>,
        is_negative_list: bool,
        input_container: Option<&str>,
    ) -> bool {
        if input_container.is_none_or(str::is_empty) {
            return is_negative_list;
        }

        Self::contains_container_span_with_polarity(
            profile_containers,
            is_negative_list,
            input_container.unwrap_or_default(),
        )
    }

    /// Matches the span overload with an explicit list polarity.
    #[must_use]
    pub fn contains_container_span_with_polarity(
        profile_containers: Option<&str>,
        is_negative_list: bool,
        input_container: &str,
    ) -> bool {
        let Some(profile_containers) = profile_containers.filter(|value| !value.is_empty()) else {
            return true;
        };

        for container in input_container.split(',').filter(|value| !value.is_empty()) {
            if profile_containers
                .split(',')
                .filter(|value| !value.is_empty())
                .any(|profile| container.eq_ignore_ascii_case(profile))
            {
                return !is_negative_list;
            }
        }

        is_negative_list
    }

    /// Matches against a pre-split profile list.
    #[must_use]
    pub fn contains_container_list(
        profile_containers: Option<&[String]>,
        is_negative_list: bool,
        input_container: &str,
    ) -> bool {
        let Some(profile_containers) = profile_containers else {
            return true;
        };

        for container in Self::split(Some(input_container)) {
            if profile_containers
                .iter()
                .any(|profile| profile.eq_ignore_ascii_case(container))
            {
                return !is_negative_list;
            }
        }

        is_negative_list
    }

    /// Splits a comma-delimited value and removes empty entries.
    #[must_use]
    pub fn split(input: Option<&str>) -> Vec<&str> {
        input
            .map(|value| value.split(',').filter(|part| !part.is_empty()).collect())
            .unwrap_or_default()
    }
}

fn split_negative_list(profile_containers: Option<&str>) -> (Option<&str>, bool) {
    match profile_containers {
        Some(value) if value.starts_with('-') => (Some(&value[1..]), true),
        value => (value, false),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum DlnaProfileType {
    #[default]
    Audio = 0,
    Video = 1,
    Photo = 2,
    Subtitle = 3,
    Lyric = 4,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum EncodingContext {
    #[default]
    Streaming = 0,
    Static = 1,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum MediaStreamProtocol {
    #[default]
    #[serde(rename = "http", alias = "")]
    Http = 0,
    #[serde(rename = "hls")]
    Hls = 1,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum TranscodeSeekInfo {
    #[default]
    Auto = 0,
    Bytes = 1,
}

impl TranscodeSeekInfo {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Bytes => "Bytes",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[repr(i32)]
pub enum PlayMethod {
    #[default]
    Transcode = 0,
    DirectStream = 1,
    DirectPlay = 2,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum VideoType {
    #[default]
    VideoFile = 0,
    Iso = 1,
    Dvd = 2,
    BluRay = 3,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum IsoType {
    #[default]
    Dvd = 0,
    BluRay = 1,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum Video3DFormat {
    #[default]
    HalfSideBySide = 0,
    FullSideBySide = 1,
    FullTopAndBottom = 2,
    HalfTopAndBottom = 3,
    MVC = 4,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum TransportStreamTimestamp {
    #[default]
    None = 0,
    Zero = 1,
    Valid = 2,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ProfileConditionType {
    #[default]
    Equals = 0,
    NotEquals = 1,
    LessThanEqual = 2,
    GreaterThanEqual = 3,
    EqualsAny = 4,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ProfileConditionValue {
    #[default]
    AudioChannels = 0,
    AudioBitrate = 1,
    AudioProfile = 2,
    Width = 3,
    Height = 4,
    Has64BitOffsets = 5,
    PacketLength = 6,
    VideoBitDepth = 7,
    VideoBitrate = 8,
    VideoFramerate = 9,
    VideoLevel = 10,
    VideoProfile = 11,
    VideoTimestamp = 12,
    IsAnamorphic = 13,
    RefFrames = 14,
    NumAudioStreams = 16,
    NumVideoStreams = 17,
    IsSecondaryAudio = 18,
    VideoCodecTag = 19,
    IsAvc = 20,
    IsInterlaced = 21,
    AudioSampleRate = 22,
    AudioBitDepth = 23,
    VideoRangeType = 24,
    NumStreams = 25,
    VideoRotation = 26,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ProfileCondition {
    pub condition: ProfileConditionType,
    pub property: ProfileConditionValue,
    pub value: String,
    pub is_required: bool,
}

impl Default for ProfileCondition {
    fn default() -> Self {
        Self {
            condition: ProfileConditionType::default(),
            property: ProfileConditionValue::default(),
            value: String::new(),
            is_required: true,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ContainerProfile {
    #[serde(rename = "Type")]
    pub profile_type: DlnaProfileType,
    pub conditions: Vec<ProfileCondition>,
    pub container: Option<String>,
    pub sub_container: Option<String>,
}

impl ContainerProfile {
    #[must_use]
    pub fn contains_container(&self, container: &str, use_sub_container: bool) -> bool {
        let profile = if use_sub_container
            && self
                .container
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("hls"))
        {
            self.sub_container.as_deref()
        } else {
            self.container.as_deref()
        };

        ContainerHelper::contains_container_span(profile, container)
    }
}

/// Media-source metadata required by stream selection and URL generation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum MediaProtocol {
    #[default]
    File = 0,
    Http = 1,
    Rtmp = 2,
    Rtsp = 3,
    Udp = 4,
    Rtp = 5,
    Ftp = 6,
}

/// The type of a media source.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum MediaSourceType {
    #[default]
    Default = 0,
    Grouping = 1,
    Placeholder = 2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MediaSourceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub protocol: MediaProtocol,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder_protocol: Option<MediaProtocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(rename = "Type")]
    pub source_type: MediaSourceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    pub is_remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    pub read_at_native_framerate: bool,
    pub ignore_dts: bool,
    pub ignore_index: bool,
    pub gen_pts_input: bool,
    pub supports_transcoding: bool,
    pub supports_direct_stream: bool,
    pub supports_direct_play: bool,
    pub is_infinite_stream: bool,
    pub use_most_compatible_transcoding_profile: bool,
    pub requires_opening: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_token: Option<String>,
    pub requires_closing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_stream_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_ms: Option<i32>,
    pub requires_looping: bool,
    pub supports_probing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_type: Option<VideoType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso_type: Option<IsoType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_3d_format: Option<Video3DFormat>,
    pub media_streams: Vec<crate::MediaStream>,
    pub media_attachments: Vec<MediaAttachment>,
    pub formats: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_max_streaming_bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<TransportStreamTimestamp>,
    pub required_http_headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcoding_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcoding_container: Option<String>,
    pub transcoding_sub_protocol: MediaStreamProtocol,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyze_duration_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_subtitle_stream_index: Option<i32>,
    pub has_segments: bool,
    #[serde(rename = "ETag", skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

impl Default for MediaSourceInfo {
    fn default() -> Self {
        Self {
            id: None,
            protocol: MediaProtocol::default(),
            path: None,
            encoder_path: None,
            encoder_protocol: None,
            name: None,
            container: None,
            source_type: MediaSourceType::default(),
            bitrate: None,
            size: None,
            is_remote: false,
            run_time_ticks: None,
            read_at_native_framerate: false,
            ignore_dts: false,
            ignore_index: false,
            gen_pts_input: false,
            supports_transcoding: true,
            supports_direct_stream: true,
            supports_direct_play: true,
            is_infinite_stream: false,
            use_most_compatible_transcoding_profile: false,
            requires_opening: false,
            open_token: None,
            requires_closing: false,
            live_stream_id: None,
            buffer_ms: None,
            requires_looping: false,
            supports_probing: true,
            video_type: None,
            iso_type: None,
            video_3d_format: None,
            media_streams: Vec::new(),
            media_attachments: Vec::new(),
            formats: Vec::new(),
            fallback_max_streaming_bitrate: None,
            timestamp: None,
            required_http_headers: HashMap::new(),
            transcoding_url: None,
            transcoding_container: None,
            transcoding_sub_protocol: MediaStreamProtocol::default(),
            analyze_duration_ms: None,
            default_audio_stream_index: None,
            default_subtitle_stream_index: None,
            has_segments: false,
            etag: None,
        }
    }
}

impl MediaSourceInfo {
    #[must_use]
    pub fn default_audio_stream(&self, default_index: Option<i32>) -> Option<&crate::MediaStream> {
        if let Some(index) = default_index.filter(|index| *index != -1)
            && let Some(stream) = self.media_streams.iter().find(|stream| {
                stream.stream_type == crate::MediaStreamType::Audio && stream.index == index
            })
        {
            return Some(stream);
        }

        self.media_streams
            .iter()
            .find(|stream| stream.stream_type == crate::MediaStreamType::Audio && stream.is_default)
            .or_else(|| {
                self.media_streams
                    .iter()
                    .find(|stream| stream.stream_type == crate::MediaStreamType::Audio)
            })
    }

    #[must_use]
    pub fn video_stream(&self) -> Option<&crate::MediaStream> {
        self.media_streams
            .iter()
            .find(|stream| stream.stream_type == crate::MediaStreamType::Video)
    }

    #[must_use]
    pub fn media_stream(
        &self,
        stream_type: crate::MediaStreamType,
        index: i32,
    ) -> Option<&crate::MediaStream> {
        self.media_streams
            .iter()
            .find(|stream| stream.stream_type == stream_type && stream.index == index)
    }
}

/// Bit flags describing why a stream must be transcoded.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TranscodeReason(u32);

impl TranscodeReason {
    pub const NONE: Self = Self(0);
    pub const CONTAINER_NOT_SUPPORTED: Self = Self(1 << 0);
    pub const VIDEO_CODEC_NOT_SUPPORTED: Self = Self(1 << 1);
    pub const AUDIO_CODEC_NOT_SUPPORTED: Self = Self(1 << 2);
    pub const SUBTITLE_CODEC_NOT_SUPPORTED: Self = Self(1 << 3);
    pub const AUDIO_IS_EXTERNAL: Self = Self(1 << 4);
    pub const SECONDARY_AUDIO_NOT_SUPPORTED: Self = Self(1 << 5);
    pub const VIDEO_PROFILE_NOT_SUPPORTED: Self = Self(1 << 6);
    pub const VIDEO_LEVEL_NOT_SUPPORTED: Self = Self(1 << 7);
    pub const VIDEO_RESOLUTION_NOT_SUPPORTED: Self = Self(1 << 8);
    pub const VIDEO_BIT_DEPTH_NOT_SUPPORTED: Self = Self(1 << 9);
    pub const VIDEO_FRAMERATE_NOT_SUPPORTED: Self = Self(1 << 10);
    pub const REF_FRAMES_NOT_SUPPORTED: Self = Self(1 << 11);
    pub const ANAMORPHIC_VIDEO_NOT_SUPPORTED: Self = Self(1 << 12);
    pub const INTERLACED_VIDEO_NOT_SUPPORTED: Self = Self(1 << 13);
    pub const AUDIO_CHANNELS_NOT_SUPPORTED: Self = Self(1 << 14);
    pub const AUDIO_PROFILE_NOT_SUPPORTED: Self = Self(1 << 15);
    pub const AUDIO_SAMPLE_RATE_NOT_SUPPORTED: Self = Self(1 << 16);
    pub const AUDIO_BIT_DEPTH_NOT_SUPPORTED: Self = Self(1 << 17);
    pub const CONTAINER_BITRATE_EXCEEDS_LIMIT: Self = Self(1 << 18);
    pub const VIDEO_BITRATE_NOT_SUPPORTED: Self = Self(1 << 19);
    pub const AUDIO_BITRATE_NOT_SUPPORTED: Self = Self(1 << 20);
    pub const UNKNOWN_VIDEO_STREAM_INFO: Self = Self(1 << 21);
    pub const UNKNOWN_AUDIO_STREAM_INFO: Self = Self(1 << 22);
    pub const DIRECT_PLAY_ERROR: Self = Self(1 << 23);
    pub const VIDEO_RANGE_TYPE_NOT_SUPPORTED: Self = Self(1 << 24);
    pub const VIDEO_CODEC_TAG_NOT_SUPPORTED: Self = Self(1 << 25);
    pub const STREAM_COUNT_EXCEEDS_LIMIT: Self = Self(1 << 26);
    pub const VIDEO_ROTATION_NOT_SUPPORTED: Self = Self(1 << 27);

    const FLAGS: [(Self, &'static str); 28] = [
        (Self::CONTAINER_NOT_SUPPORTED, "ContainerNotSupported"),
        (Self::VIDEO_CODEC_NOT_SUPPORTED, "VideoCodecNotSupported"),
        (Self::AUDIO_CODEC_NOT_SUPPORTED, "AudioCodecNotSupported"),
        (
            Self::SUBTITLE_CODEC_NOT_SUPPORTED,
            "SubtitleCodecNotSupported",
        ),
        (Self::AUDIO_IS_EXTERNAL, "AudioIsExternal"),
        (
            Self::SECONDARY_AUDIO_NOT_SUPPORTED,
            "SecondaryAudioNotSupported",
        ),
        (
            Self::VIDEO_PROFILE_NOT_SUPPORTED,
            "VideoProfileNotSupported",
        ),
        (Self::VIDEO_LEVEL_NOT_SUPPORTED, "VideoLevelNotSupported"),
        (
            Self::VIDEO_RESOLUTION_NOT_SUPPORTED,
            "VideoResolutionNotSupported",
        ),
        (
            Self::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
            "VideoBitDepthNotSupported",
        ),
        (
            Self::VIDEO_FRAMERATE_NOT_SUPPORTED,
            "VideoFramerateNotSupported",
        ),
        (Self::REF_FRAMES_NOT_SUPPORTED, "RefFramesNotSupported"),
        (
            Self::ANAMORPHIC_VIDEO_NOT_SUPPORTED,
            "AnamorphicVideoNotSupported",
        ),
        (
            Self::INTERLACED_VIDEO_NOT_SUPPORTED,
            "InterlacedVideoNotSupported",
        ),
        (
            Self::AUDIO_CHANNELS_NOT_SUPPORTED,
            "AudioChannelsNotSupported",
        ),
        (
            Self::AUDIO_PROFILE_NOT_SUPPORTED,
            "AudioProfileNotSupported",
        ),
        (
            Self::AUDIO_SAMPLE_RATE_NOT_SUPPORTED,
            "AudioSampleRateNotSupported",
        ),
        (
            Self::AUDIO_BIT_DEPTH_NOT_SUPPORTED,
            "AudioBitDepthNotSupported",
        ),
        (
            Self::CONTAINER_BITRATE_EXCEEDS_LIMIT,
            "ContainerBitrateExceedsLimit",
        ),
        (
            Self::VIDEO_BITRATE_NOT_SUPPORTED,
            "VideoBitrateNotSupported",
        ),
        (
            Self::AUDIO_BITRATE_NOT_SUPPORTED,
            "AudioBitrateNotSupported",
        ),
        (Self::UNKNOWN_VIDEO_STREAM_INFO, "UnknownVideoStreamInfo"),
        (Self::UNKNOWN_AUDIO_STREAM_INFO, "UnknownAudioStreamInfo"),
        (Self::DIRECT_PLAY_ERROR, "DirectPlayError"),
        (
            Self::VIDEO_RANGE_TYPE_NOT_SUPPORTED,
            "VideoRangeTypeNotSupported",
        ),
        (
            Self::VIDEO_CODEC_TAG_NOT_SUPPORTED,
            "VideoCodecTagNotSupported",
        ),
        (Self::STREAM_COUNT_EXCEEDS_LIMIT, "StreamCountExceedsLimit"),
        (
            Self::VIDEO_ROTATION_NOT_SUPPORTED,
            "VideoRotationNotSupported",
        ),
    ];

    #[must_use]
    pub const fn from_bits_retain(bits: u32) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
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

    fn names(self) -> impl Iterator<Item = &'static str> {
        Self::FLAGS
            .into_iter()
            .filter(move |(flag, _)| self.contains(*flag))
            .map(|(_, name)| name)
    }
}

impl std::ops::BitOr for TranscodeReason {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for TranscodeReason {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Information used to build Jellyfin playback URLs.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub item_id: Uuid,
    pub play_method: PlayMethod,
    pub context: EncodingContext,
    pub media_type: DlnaProfileType,
    pub container: Option<String>,
    pub sub_protocol: MediaStreamProtocol,
    pub start_position_ticks: i64,
    pub segment_length: Option<i32>,
    pub min_segments: Option<i32>,
    pub require_avc: bool,
    pub require_non_anamorphic: bool,
    pub copy_timestamps: bool,
    pub enable_mpegts_m2ts_mode: bool,
    pub enable_subtitles_in_manifest: bool,
    pub audio_codecs: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub transcoding_max_audio_channels: Option<i32>,
    pub global_max_audio_channels: Option<i32>,
    pub audio_bitrate: Option<i32>,
    pub audio_sample_rate: Option<i32>,
    pub video_bitrate: Option<i32>,
    pub max_width: Option<i32>,
    pub max_height: Option<i32>,
    pub max_framerate: Option<f32>,
    pub device_profile_id: Option<String>,
    pub device_id: Option<String>,
    pub run_time_ticks: Option<i64>,
    pub transcode_seek_info: TranscodeSeekInfo,
    pub estimate_content_length: bool,
    pub media_source: Option<MediaSourceInfo>,
    pub subtitle_codecs: Vec<String>,
    pub subtitle_delivery_method: SubtitleDeliveryMethod,
    pub subtitle_format: Option<String>,
    pub play_session_id: Option<String>,
    pub transcode_reasons: TranscodeReason,
    pub enable_audio_vbr_encoding: bool,
    pub always_burn_in_subtitle_when_transcoding: bool,
    stream_options: Vec<(String, String)>,
}

impl Default for StreamInfo {
    fn default() -> Self {
        Self {
            item_id: Uuid::nil(),
            play_method: PlayMethod::default(),
            context: EncodingContext::default(),
            media_type: DlnaProfileType::default(),
            container: None,
            sub_protocol: MediaStreamProtocol::default(),
            start_position_ticks: 0,
            segment_length: None,
            min_segments: None,
            require_avc: false,
            require_non_anamorphic: false,
            copy_timestamps: false,
            enable_mpegts_m2ts_mode: false,
            enable_subtitles_in_manifest: false,
            audio_codecs: Vec::new(),
            video_codecs: Vec::new(),
            audio_stream_index: None,
            subtitle_stream_index: None,
            transcoding_max_audio_channels: None,
            global_max_audio_channels: None,
            audio_bitrate: None,
            audio_sample_rate: None,
            video_bitrate: None,
            max_width: None,
            max_height: None,
            max_framerate: None,
            device_profile_id: None,
            device_id: None,
            run_time_ticks: None,
            transcode_seek_info: TranscodeSeekInfo::default(),
            estimate_content_length: false,
            media_source: None,
            subtitle_codecs: Vec::new(),
            subtitle_delivery_method: SubtitleDeliveryMethod::default(),
            subtitle_format: None,
            play_session_id: None,
            transcode_reasons: TranscodeReason::default(),
            enable_audio_vbr_encoding: false,
            always_burn_in_subtitle_when_transcoding: false,
            stream_options: Vec::new(),
        }
    }
}

impl StreamInfo {
    #[must_use]
    pub fn new(item_id: Uuid, media_type: DlnaProfileType) -> Self {
        Self {
            item_id,
            media_type,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn media_source_id(&self) -> Option<&str> {
        self.media_source.as_ref()?.id.as_deref()
    }

    #[must_use]
    pub fn target_audio_stream(&self) -> Option<&crate::MediaStream> {
        self.media_source
            .as_ref()?
            .default_audio_stream(self.audio_stream_index)
    }

    #[must_use]
    pub fn target_video_stream(&self) -> Option<&crate::MediaStream> {
        self.media_source.as_ref()?.video_stream()
    }

    #[must_use]
    pub fn target_audio_codecs(&self) -> Vec<&str> {
        let input_codec = self
            .target_audio_stream()
            .and_then(|stream| stream.codec.as_deref());
        if self.is_direct_stream() {
            return input_codec.into_iter().collect();
        }

        if let Some(input_codec) = input_codec
            && let Some(codec) = self
                .audio_codecs
                .iter()
                .find(|codec| codec.eq_ignore_ascii_case(input_codec))
        {
            return vec![codec];
        }

        self.audio_codecs.iter().map(String::as_str).collect()
    }

    #[must_use]
    pub fn target_audio_channels(&self, codec: Option<&str>) -> Option<i32> {
        if self.is_direct_stream() {
            return self
                .target_audio_stream()
                .and_then(|stream| stream.channels);
        }

        let default_value = self
            .global_max_audio_channels
            .or(self.transcoding_max_audio_channels);
        let value = self
            .get_qualified_option(codec, "audiochannels")
            .and_then(|value| value.parse::<i32>().ok());
        match (value, default_value) {
            (Some(value), Some(maximum)) => Some(value.min(maximum)),
            (Some(value), None) => Some(value),
            (None, default_value) => default_value,
        }
    }

    #[must_use]
    pub fn is_direct_stream(&self) -> bool {
        let is_disc = self
            .media_source
            .as_ref()
            .and_then(|source| source.video_type)
            .is_some_and(|video_type| matches!(video_type, VideoType::Dvd | VideoType::BluRay));

        !is_disc
            && matches!(
                self.play_method,
                PlayMethod::DirectStream | PlayMethod::DirectPlay
            )
    }

    pub fn set_option(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        if let Some((_, existing_value)) = self
            .stream_options
            .iter_mut()
            .find(|(existing_name, _)| existing_name.eq_ignore_ascii_case(&name))
        {
            *existing_value = value;
        } else {
            self.stream_options.push((name, value));
        }
    }

    pub fn set_qualified_option(
        &mut self,
        qualifier: Option<&str>,
        name: &str,
        value: impl Into<String>,
    ) {
        if let Some(qualifier) = qualifier.filter(|value| !value.is_empty()) {
            self.set_option(format!("{qualifier}-{name}"), value);
        } else {
            self.set_option(name, value);
        }
    }

    #[must_use]
    pub fn get_option(&self, name: &str) -> Option<&str> {
        self.stream_options
            .iter()
            .find(|(existing_name, _)| existing_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[must_use]
    pub fn get_qualified_option(&self, qualifier: Option<&str>, name: &str) -> Option<&str> {
        let qualified_name = format!("{}-{name}", qualifier.unwrap_or_default());
        self.get_option(&qualified_name)
            .filter(|value| !value.is_empty())
            .or_else(|| self.get_option(name))
    }

    #[must_use]
    pub fn stream_options(&self) -> &[(String, String)] {
        &self.stream_options
    }

    /// Builds the playback URL using Jellyfin's stable parameter ordering.
    #[must_use]
    pub fn to_url(
        &self,
        base_url: Option<&str>,
        access_token: Option<&str>,
        query: Option<&str>,
    ) -> String {
        let mut url = base_url
            .filter(|value| !value.is_empty())
            .map_or_else(String::new, |value| value.trim_end_matches('/').to_owned());

        if self.media_type == DlnaProfileType::Audio {
            url.push_str("/audio/");
        } else {
            url.push_str("/videos/");
        }
        write!(url, "{}", self.item_id).expect("writing to a string cannot fail");

        if self.sub_protocol == MediaStreamProtocol::Hls {
            url.push_str("/master.m3u8");
        } else {
            url.push_str("/stream");
            if let Some(container) = self.container.as_deref().filter(|value| !value.is_empty()) {
                url.push('.');
                url.push_str(container);
            }
        }

        let query_start = url.len();
        append_non_empty(
            &mut url,
            "DeviceProfileId",
            self.device_profile_id.as_deref(),
        );
        append_non_empty(&mut url, "DeviceId", self.device_id.as_deref());
        append_non_empty(&mut url, "MediaSourceId", self.media_source_id());

        if self.is_direct_stream() {
            url.push_str("&Static=true");
        }
        append_joined(&mut url, "VideoCodec", &self.video_codecs);
        append_joined(&mut url, "AudioCodec", &self.audio_codecs);
        append_number(&mut url, "AudioStreamIndex", self.audio_stream_index);

        if let Some(index) = self.subtitle_stream_index.filter(|index| {
            *index != -1
                && (self.always_burn_in_subtitle_when_transcoding
                    || self.subtitle_delivery_method != SubtitleDeliveryMethod::External)
        }) {
            append_value(&mut url, "SubtitleStreamIndex", index);
        }

        append_number(&mut url, "VideoBitrate", self.video_bitrate);
        append_number(&mut url, "AudioBitrate", self.audio_bitrate);
        append_number(&mut url, "AudioSampleRate", self.audio_sample_rate);
        if let Some(max_framerate) = self.max_framerate {
            append_value(&mut url, "MaxFramerate", max_framerate);
        }
        append_number(&mut url, "MaxWidth", self.max_width);
        append_number(&mut url, "MaxHeight", self.max_height);

        if self.sub_protocol == MediaStreamProtocol::Hls {
            append_non_empty(&mut url, "SegmentContainer", self.container.as_deref());
            append_number(&mut url, "SegmentLength", self.segment_length);
            append_number(&mut url, "MinSegments", self.min_segments);
        } else if self.start_position_ticks != 0 {
            append_value(&mut url, "StartTimeTicks", self.start_position_ticks);
        }

        append_non_empty(&mut url, "PlaySessionId", self.play_session_id.as_deref());
        append_non_empty(&mut url, "ApiKey", access_token);
        if let Some(source) = &self.media_source {
            append_non_empty(&mut url, "LiveStreamId", source.live_stream_id.as_deref());
        }

        if !self.is_direct_stream() {
            if self.require_non_anamorphic {
                url.push_str("&RequireNonAnamorphic=True");
            }
            append_number(
                &mut url,
                "TranscodingMaxAudioChannels",
                self.transcoding_max_audio_channels,
            );
            if self.enable_subtitles_in_manifest {
                url.push_str("&EnableSubtitlesInManifest=True");
            }
            if self.enable_mpegts_m2ts_mode {
                url.push_str("&EnableMpegtsM2TsMode=True");
            }
            if self.estimate_content_length {
                url.push_str("&EstimateContentLength=True");
            }
            if self.transcode_seek_info != TranscodeSeekInfo::Auto {
                append_value(
                    &mut url,
                    "TranscodeSeekInfo",
                    self.transcode_seek_info.as_str(),
                );
            }
            if self.copy_timestamps {
                url.push_str("&CopyTimestamps=True");
            }
            append_value(&mut url, "RequireAvc", self.require_avc);
            append_value(
                &mut url,
                "EnableAudioVbrEncoding",
                self.enable_audio_vbr_encoding,
            );
        }

        if let Some(source) = &self.media_source {
            append_non_empty(&mut url, "Tag", source.etag.as_deref());
        }
        if self.subtitle_stream_index.is_some()
            && self.subtitle_delivery_method != SubtitleDeliveryMethod::External
        {
            append_value(
                &mut url,
                "SubtitleMethod",
                subtitle_delivery_method_name(self.subtitle_delivery_method),
            );
        }
        if self.subtitle_stream_index.is_some()
            && self.subtitle_delivery_method == SubtitleDeliveryMethod::Embed
        {
            append_joined(&mut url, "SubtitleCodec", &self.subtitle_codecs);
        }

        for (name, value) in &self.stream_options {
            url.push('&');
            url.push_str(name);
            url.push('=');
            url.extend(value.chars().filter(|character| *character != ' '));
        }

        if !self.is_direct_stream() && !self.transcode_reasons.is_empty() {
            url.push_str("&TranscodeReasons=");
            for (index, reason) in self.transcode_reasons.names().enumerate() {
                if index != 0 {
                    url.push(',');
                }
                url.push_str(reason);
            }
        }

        if let Some(query) = query.filter(|value| !value.is_empty()) {
            url.push_str(query);
        }
        if url.len() > query_start {
            let first_character_length =
                url[query_start..].chars().next().map_or(0, char::len_utf8);
            url.replace_range(query_start..query_start + first_character_length, "?");
        }

        url
    }
}

fn append_non_empty(url: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        append_value(url, name, value);
    }
}

fn append_number<T: std::fmt::Display>(url: &mut String, name: &str, value: Option<T>) {
    if let Some(value) = value {
        append_value(url, name, value);
    }
}

fn append_joined(url: &mut String, name: &str, values: &[String]) {
    if !values.is_empty() {
        url.push('&');
        url.push_str(name);
        url.push('=');
        for (index, value) in values.iter().enumerate() {
            if index != 0 {
                url.push(',');
            }
            url.push_str(value);
        }
    }
}

fn append_value(url: &mut String, name: &str, value: impl std::fmt::Display) {
    url.push('&');
    url.push_str(name);
    url.push('=');
    write!(url, "{value}").expect("writing to a string cannot fail");
}

fn subtitle_delivery_method_name(method: SubtitleDeliveryMethod) -> &'static str {
    match method {
        SubtitleDeliveryMethod::Encode => "Encode",
        SubtitleDeliveryMethod::Embed => "Embed",
        SubtitleDeliveryMethod::External => "External",
        SubtitleDeliveryMethod::Hls => "Hls",
        SubtitleDeliveryMethod::Drop => "Drop",
    }
}
