use std::cmp::Ordering;

use uuid::Uuid;

use super::{
    ContainerHelper, DlnaProfileType, EncodingContext, MediaProtocol, MediaSourceInfo,
    MediaStreamProtocol, PlayMethod, ProfileCondition, ProfileConditionType, ProfileConditionValue,
    StreamInfo, TranscodeReason, TranscodeSeekInfo,
};
use crate::MediaStream;

const HLS_AUDIO_CODECS_TS: &[&str] = &["aac", "ac3", "eac3", "mp3"];
const HLS_AUDIO_CODECS_MP4: &[&str] = &[
    "aac", "ac3", "eac3", "mp3", "alac", "flac", "opus", "dts", "truehd",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirectPlayProfile {
    pub container: String,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub profile_type: DlnaProfileType,
}

impl DirectPlayProfile {
    #[must_use]
    pub fn supports_container(&self, container: Option<&str>) -> bool {
        ContainerHelper::contains_container(Some(&self.container), container)
    }

    #[must_use]
    pub fn supports_audio_codec(&self, codec: Option<&str>) -> bool {
        matches!(
            self.profile_type,
            DlnaProfileType::Audio | DlnaProfileType::Video
        ) && ContainerHelper::contains_container(self.audio_codec.as_deref(), codec)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CodecType {
    #[default]
    Video = 0,
    VideoAudio = 1,
    Audio = 2,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodecProfile {
    pub profile_type: CodecType,
    pub conditions: Vec<ProfileCondition>,
    pub apply_conditions: Vec<ProfileCondition>,
    pub codec: Option<String>,
    pub container: Option<String>,
    pub sub_container: Option<String>,
}

impl CodecProfile {
    fn contains_audio_codec(&self, codec: Option<&str>, container: Option<&str>) -> bool {
        ContainerHelper::contains_container(self.container.as_deref(), container)
            && ContainerHelper::contains_container_span_with_polarity(
                self.codec.as_deref(),
                false,
                codec.unwrap_or_default(),
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodingProfile {
    pub container: String,
    pub profile_type: DlnaProfileType,
    pub video_codec: String,
    pub audio_codec: String,
    pub protocol: MediaStreamProtocol,
    pub estimate_content_length: bool,
    pub enable_mpegts_m2ts_mode: bool,
    pub transcode_seek_info: TranscodeSeekInfo,
    pub copy_timestamps: bool,
    pub context: EncodingContext,
    pub enable_subtitles_in_manifest: bool,
    pub max_audio_channels: Option<String>,
    pub min_segments: i32,
    pub segment_length: i32,
    pub conditions: Vec<ProfileCondition>,
    pub enable_audio_vbr_encoding: bool,
}

impl Default for TranscodingProfile {
    fn default() -> Self {
        Self {
            container: String::new(),
            profile_type: DlnaProfileType::default(),
            video_codec: String::new(),
            audio_codec: String::new(),
            protocol: MediaStreamProtocol::default(),
            estimate_content_length: false,
            enable_mpegts_m2ts_mode: false,
            transcode_seek_info: TranscodeSeekInfo::default(),
            copy_timestamps: false,
            context: EncodingContext::default(),
            enable_subtitles_in_manifest: false,
            max_audio_channels: None,
            min_segments: 0,
            segment_length: 0,
            conditions: Vec::new(),
            enable_audio_vbr_encoding: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProfile {
    pub name: Option<String>,
    pub id: Option<Uuid>,
    pub max_streaming_bitrate: Option<i32>,
    pub max_static_bitrate: Option<i32>,
    pub music_streaming_transcoding_bitrate: Option<i32>,
    pub max_static_music_bitrate: Option<i32>,
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    pub transcoding_profiles: Vec<TranscodingProfile>,
    pub container_profiles: Vec<super::ContainerProfile>,
    pub codec_profiles: Vec<CodecProfile>,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        Self {
            name: None,
            id: None,
            max_streaming_bitrate: Some(8_000_000),
            max_static_bitrate: Some(8_000_000),
            music_streaming_transcoding_bitrate: Some(128_000),
            max_static_music_bitrate: Some(8_000_000),
            direct_play_profiles: Vec::new(),
            transcoding_profiles: Vec::new(),
            container_profiles: Vec::new(),
            codec_profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaOptions {
    pub enable_direct_play: bool,
    pub enable_direct_stream: bool,
    pub force_direct_play: bool,
    pub force_direct_stream: bool,
    pub allow_audio_stream_copy: bool,
    pub item_id: Uuid,
    pub media_sources: Vec<MediaSourceInfo>,
    pub profile: DeviceProfile,
    pub media_source_id: Option<String>,
    pub device_id: Option<String>,
    pub max_audio_channels: Option<i32>,
    pub max_bitrate: Option<i32>,
    pub context: EncodingContext,
    pub audio_transcoding_bitrate: Option<i32>,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self {
            enable_direct_play: true,
            enable_direct_stream: true,
            force_direct_play: false,
            force_direct_stream: false,
            allow_audio_stream_copy: false,
            item_id: Uuid::nil(),
            media_sources: Vec::new(),
            profile: DeviceProfile::default(),
            media_source_id: None,
            device_id: None,
            max_audio_channels: None,
            max_bitrate: None,
            context: EncodingContext::default(),
            audio_transcoding_bitrate: None,
        }
    }
}

impl MediaOptions {
    #[must_use]
    pub fn max_bitrate(&self, is_audio: bool) -> Option<i32> {
        if self.max_bitrate.is_some() {
            return self.max_bitrate;
        }
        if self.context == EncodingContext::Static {
            if is_audio && self.profile.max_static_music_bitrate.is_some() {
                return self.profile.max_static_music_bitrate;
            }
            return self.profile.max_static_bitrate;
        }
        self.profile.max_streaming_bitrate
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamBuilderError {
    MissingDeviceId,
    MissingAudioStream,
}

impl std::fmt::Display for StreamBuilderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDeviceId => {
                formatter.write_str("device id is required for an empty item id")
            }
            Self::MissingAudioStream => formatter.write_str("media source has no audio stream"),
        }
    }
}

impl std::error::Error for StreamBuilderError {}

/// Pure DLNA stream selection; codec capability is supplied by the caller.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamBuilder {
    encodable_audio_codecs: Option<Vec<String>>,
}

impl StreamBuilder {
    #[must_use]
    pub fn with_encodable_audio_codecs<I, S>(codecs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            encodable_audio_codecs: Some(codecs.into_iter().map(Into::into).collect()),
        }
    }

    pub fn get_optimal_audio_stream(
        &self,
        options: &MediaOptions,
    ) -> Result<Option<StreamInfo>, StreamBuilderError> {
        if options.item_id.is_nil()
            && options
                .device_id
                .as_deref()
                .is_none_or(|device_id| device_id.is_empty())
        {
            return Err(StreamBuilderError::MissingDeviceId);
        }

        let max_bitrate = i64::from(options.max_bitrate(true).unwrap_or_default());
        let mut streams = Vec::new();
        for (index, source) in options.media_sources.iter().enumerate() {
            if let Some(requested_id) = options
                .media_source_id
                .as_deref()
                .filter(|requested_id| !requested_id.is_empty())
                && !source
                    .id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(requested_id))
            {
                continue;
            }

            if let Some(mut stream) = self.build_audio_stream(source, options)? {
                stream.device_id.clone_from(&options.device_id);
                stream.device_profile_id = options.profile.id.map(|id| id.simple().to_string());
                streams.push((index, stream));
            }
        }

        streams.sort_by(|(left_index, left), (right_index, right)| {
            audio_stream_rank(left, max_bitrate, *left_index).cmp(&audio_stream_rank(
                right,
                max_bitrate,
                *right_index,
            ))
        });
        Ok(streams.into_iter().next().map(|(_, stream)| stream))
    }

    fn build_audio_stream(
        &self,
        source: &MediaSourceInfo,
        options: &MediaOptions,
    ) -> Result<Option<StreamInfo>, StreamBuilderError> {
        let mut stream = StreamInfo::new(options.item_id, DlnaProfileType::Audio);
        stream.media_source = Some(source.clone());
        stream.run_time_ticks = source.run_time_ticks;
        stream.context = options.context;

        if options.force_direct_play {
            stream.play_method = PlayMethod::DirectPlay;
            stream.container = normalize_container(
                source.container.as_deref(),
                &options.profile,
                DlnaProfileType::Audio,
                None,
            );
            return Ok(Some(stream));
        }
        if options.force_direct_stream {
            stream.play_method = PlayMethod::DirectStream;
            stream.container = normalize_container(
                source.container.as_deref(),
                &options.profile,
                DlnaProfileType::Audio,
                None,
            );
            return Ok(Some(stream));
        }

        let audio_stream = source
            .default_audio_stream(None)
            .ok_or(StreamBuilderError::MissingAudioStream)?;
        let direct = audio_direct_play_profile(source, audio_stream, options);
        let mut reasons = direct.reasons;

        if direct.method == Some(PlayMethod::DirectPlay) {
            let failures = compatibility_audio_codec(
                &options.profile,
                source.container.as_deref(),
                audio_stream,
            );
            reasons |= failures;
            if failures.is_empty() {
                stream.play_method = PlayMethod::DirectPlay;
                stream.container = normalize_container(
                    source.container.as_deref(),
                    &options.profile,
                    DlnaProfileType::Audio,
                    direct.profile,
                );
                return Ok(Some(stream));
            }
        }

        if direct.method == Some(PlayMethod::DirectStream) {
            let profile = direct.profile.expect("direct stream always has a profile");
            let mut remux_container = source
                .transcoding_container
                .as_deref()
                .unwrap_or("ts")
                .to_owned();
            if profile.container.eq_ignore_ascii_case("ts")
                || profile.container.eq_ignore_ascii_case("mp4")
            {
                remux_container.clone_from(&profile.container);
            }
            let profile_codec = profile.audio_codec.as_deref().unwrap_or(&profile.container);
            let codec_supported = source.transcoding_sub_protocol != MediaStreamProtocol::Hls
                || if remux_container.eq_ignore_ascii_case("mp4") {
                    HLS_AUDIO_CODECS_MP4.contains(&profile_codec)
                } else {
                    HLS_AUDIO_CODECS_TS.contains(&profile_codec)
                };

            if codec_supported {
                stream.play_method = PlayMethod::DirectStream;
                stream.container = Some(remux_container);
                stream.transcode_reasons = reasons;
                stream.sub_protocol = source.transcoding_sub_protocol;
                return Ok(Some(stream));
            }
            reasons |= TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED;
        }

        let transcoding_profile = options.profile.transcoding_profiles.iter().find(|profile| {
            profile.profile_type == DlnaProfileType::Audio
                && profile.context == options.context
                && self.can_encode_audio(profile)
        });
        if let Some(profile) = transcoding_profile {
            if !source.supports_transcoding {
                return Ok(None);
            }
            apply_transcoding_profile(&mut stream, profile);
            apply_audio_transcoding_conditions(
                &mut stream,
                &options.profile.codec_profiles,
                profile,
                audio_stream,
            );
            stream.global_max_audio_channels = options.max_audio_channels;

            let configured_bitrate = options.max_bitrate(true).map(i64::from);
            let mut transcoding_bitrate = options
                .audio_transcoding_bitrate
                .map(i64::from)
                .or_else(|| {
                    (options.context == EncodingContext::Streaming)
                        .then_some(options.profile.music_streaming_transcoding_bitrate)
                        .flatten()
                        .map(i64::from)
                })
                .or(configured_bitrate)
                .unwrap_or(128_000);
            if let Some(configured_bitrate) = configured_bitrate {
                transcoding_bitrate = transcoding_bitrate.min(configured_bitrate);
            }
            let profile_bitrate = stream.audio_bitrate.map_or(transcoding_bitrate, i64::from);
            let bitrate = transcoding_bitrate.min(profile_bitrate);
            stream.audio_bitrate =
                Some(bitrate.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
            if stream.audio_codecs.is_empty() && !profile.audio_codec.trim().is_empty() {
                stream.audio_codecs.push(profile.audio_codec.clone());
            }
        }

        stream.transcode_reasons = reasons;
        Ok(Some(stream))
    }

    fn can_encode_audio(&self, profile: &TranscodingProfile) -> bool {
        let codec = &profile.audio_codec;
        self.encodable_audio_codecs.as_ref().map_or_else(
            || !codec.is_empty(),
            |supported| {
                supported
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(codec))
            },
        )
    }
}

#[derive(Clone, Copy)]
struct DirectPlayDecision<'a> {
    profile: Option<&'a DirectPlayProfile>,
    method: Option<PlayMethod>,
    reasons: TranscodeReason,
}

fn audio_direct_play_profile<'a>(
    source: &MediaSourceInfo,
    audio_stream: &MediaStream,
    options: &'a MediaOptions,
) -> DirectPlayDecision<'a> {
    let mut profile = options.profile.direct_play_profiles.iter().find(|profile| {
        profile.profile_type == DlnaProfileType::Audio
            && is_audio_direct_play_supported(profile, source, audio_stream)
    });
    let mut reasons = TranscodeReason::NONE;
    if profile.is_none() {
        profile = options.profile.direct_play_profiles.iter().find(|profile| {
            profile.profile_type == DlnaProfileType::Audio
                && is_audio_direct_stream_supported(profile, source, audio_stream)
        });
        if profile.is_some() {
            reasons |= TranscodeReason::CONTAINER_NOT_SUPPORTED;
        } else {
            return DirectPlayDecision {
                profile: None,
                method: None,
                reasons: direct_profile_failure_reasons(
                    source,
                    audio_stream,
                    &options.profile.direct_play_profiles,
                ),
            };
        }
    }

    if source.supports_direct_play && reasons.is_empty() {
        if !bitrate_limit_exceeded(source, options.max_bitrate(true).unwrap_or_default()) {
            if options.enable_direct_play {
                return DirectPlayDecision {
                    profile,
                    method: Some(PlayMethod::DirectPlay),
                    reasons: TranscodeReason::NONE,
                };
            }
        } else {
            reasons |= TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT;
        }
    }

    if source.supports_direct_stream {
        if !bitrate_limit_exceeded(source, options.max_bitrate(true).unwrap_or_default()) {
            if reasons == TranscodeReason::CONTAINER_NOT_SUPPORTED {
                return DirectPlayDecision {
                    profile,
                    method: Some(PlayMethod::DirectStream),
                    reasons,
                };
            }
        } else {
            reasons |= TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT;
        }
    }

    DirectPlayDecision {
        profile,
        method: None,
        reasons,
    }
}

fn is_audio_container_supported(profile: &DirectPlayProfile, source: &MediaSourceInfo) -> bool {
    profile.supports_container(source.container.as_deref())
        && (!ContainerHelper::contains_container(Some("mkv"), source.container.as_deref())
            || profile.supports_container(Some("mkv")))
}

fn is_audio_direct_play_supported(
    profile: &DirectPlayProfile,
    source: &MediaSourceInfo,
    audio_stream: &MediaStream,
) -> bool {
    is_audio_container_supported(profile, source)
        && profile.supports_audio_codec(audio_stream.codec.as_deref())
}

fn is_audio_direct_stream_supported(
    profile: &DirectPlayProfile,
    source: &MediaSourceInfo,
    audio_stream: &MediaStream,
) -> bool {
    if is_audio_container_supported(profile, source) {
        return false;
    }
    let codec = audio_stream.codec.as_deref();
    option_eq_ignore_ascii_case(profile.audio_codec.as_deref(), codec)
        || option_eq_ignore_ascii_case(Some(&profile.container), codec)
}

fn option_eq_ignore_ascii_case(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

fn direct_profile_failure_reasons(
    source: &MediaSourceInfo,
    audio_stream: &MediaStream,
    profiles: &[DirectPlayProfile],
) -> TranscodeReason {
    let mut container_supported = false;
    let mut audio_supported = false;
    let mut video_supported = false;
    for profile in profiles {
        if profile.profile_type == DlnaProfileType::Audio
            && profile.supports_container(source.container.as_deref())
        {
            container_supported = true;
            video_supported = true;
            audio_supported = profile.supports_audio_codec(audio_stream.codec.as_deref());
            if audio_supported {
                break;
            }
        }
    }
    let mut reasons = TranscodeReason::NONE;
    if !container_supported {
        reasons |= TranscodeReason::CONTAINER_NOT_SUPPORTED;
    }
    if !video_supported {
        reasons |= TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED;
    }
    if !audio_supported {
        reasons |= TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED;
    }
    reasons
}

fn bitrate_limit_exceeded(source: &MediaSourceInfo, max_bitrate: i32) -> bool {
    if source.is_remote {
        return false;
    }
    let requested_max = if max_bitrate > 0 {
        max_bitrate
    } else {
        i32::MAX
    };
    source.bitrate.unwrap_or(40_000_000) > requested_max
}

fn compatibility_audio_codec(
    profile: &DeviceProfile,
    container: Option<&str>,
    audio_stream: &MediaStream,
) -> TranscodeReason {
    profile
        .codec_profiles
        .iter()
        .filter(|codec_profile| {
            codec_profile.profile_type == CodecType::Audio
                && codec_profile.contains_audio_codec(audio_stream.codec.as_deref(), container)
                && codec_profile
                    .apply_conditions
                    .iter()
                    .all(|condition| audio_condition_satisfied(condition, audio_stream))
        })
        .flat_map(|codec_profile| &codec_profile.conditions)
        .filter(|condition| !audio_condition_satisfied(condition, audio_stream))
        .fold(TranscodeReason::NONE, |reasons, condition| {
            reasons | transcode_reason_for_failed_condition(condition.property)
        })
}

fn audio_condition_satisfied(condition: &ProfileCondition, stream: &MediaStream) -> bool {
    let current = match condition.property {
        ProfileConditionValue::AudioBitrate => stream.bit_rate,
        ProfileConditionValue::AudioChannels => stream.channels,
        ProfileConditionValue::AudioSampleRate => stream.sample_rate,
        ProfileConditionValue::AudioBitDepth => stream.bit_depth,
        _ => return false,
    };
    let Some(current) = current else {
        return !condition.is_required;
    };
    if condition.condition == ProfileConditionType::EqualsAny {
        return condition
            .value
            .split('|')
            .filter_map(|value| value.parse::<i32>().ok())
            .any(|value| value == current);
    }
    let Ok(expected) = condition.value.parse::<i32>() else {
        return false;
    };
    match condition.condition {
        ProfileConditionType::Equals => current == expected,
        ProfileConditionType::NotEquals => current != expected,
        ProfileConditionType::LessThanEqual => current <= expected,
        ProfileConditionType::GreaterThanEqual => current >= expected,
        ProfileConditionType::EqualsAny => false,
    }
}

const fn transcode_reason_for_failed_condition(property: ProfileConditionValue) -> TranscodeReason {
    match property {
        ProfileConditionValue::AudioBitrate => TranscodeReason::AUDIO_BITRATE_NOT_SUPPORTED,
        ProfileConditionValue::AudioChannels => TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED,
        ProfileConditionValue::AudioProfile => TranscodeReason::AUDIO_PROFILE_NOT_SUPPORTED,
        ProfileConditionValue::AudioSampleRate => TranscodeReason::AUDIO_SAMPLE_RATE_NOT_SUPPORTED,
        ProfileConditionValue::AudioBitDepth => TranscodeReason::AUDIO_BIT_DEPTH_NOT_SUPPORTED,
        _ => TranscodeReason::NONE,
    }
}

fn apply_transcoding_profile(stream: &mut StreamInfo, profile: &TranscodingProfile) {
    stream.container = Some(profile.container.clone());
    stream.sub_protocol = profile.protocol;
    stream.transcode_seek_info = profile.transcode_seek_info;
    stream.transcoding_max_audio_channels = profile
        .max_audio_channels
        .as_deref()
        .and_then(|value| value.parse::<i32>().ok());
    stream.estimate_content_length = profile.estimate_content_length;
    stream.copy_timestamps = profile.copy_timestamps;
    stream.enable_subtitles_in_manifest = profile.enable_subtitles_in_manifest;
    stream.enable_mpegts_m2ts_mode = profile.enable_mpegts_m2ts_mode;
    stream.enable_audio_vbr_encoding = profile.enable_audio_vbr_encoding;
    stream.min_segments = (profile.min_segments > 0).then_some(profile.min_segments);
    stream.segment_length = (profile.segment_length > 0).then_some(profile.segment_length);
}

fn apply_audio_transcoding_conditions(
    stream: &mut StreamInfo,
    codec_profiles: &[CodecProfile],
    transcode_profile: &TranscodingProfile,
    input: &MediaStream,
) {
    for condition in codec_profiles
        .iter()
        .filter(|profile| {
            profile.profile_type == CodecType::Audio
                && profile.contains_audio_codec(
                    Some(&transcode_profile.audio_codec),
                    Some(&transcode_profile.container),
                )
                && profile
                    .apply_conditions
                    .iter()
                    .all(|condition| audio_condition_satisfied(condition, input))
        })
        .flat_map(|profile| &profile.conditions)
    {
        if condition.value.is_empty()
            || condition.condition == ProfileConditionType::GreaterThanEqual
        {
            continue;
        }
        let Ok(value) = condition.value.parse::<i32>() else {
            continue;
        };
        match condition.property {
            ProfileConditionValue::AudioBitrate => {
                stream.audio_bitrate =
                    apply_numeric_condition(stream.audio_bitrate, value, condition.condition);
            }
            ProfileConditionValue::AudioSampleRate => {
                stream.audio_sample_rate =
                    apply_numeric_condition(stream.audio_sample_rate, value, condition.condition);
            }
            ProfileConditionValue::AudioChannels => {
                let current = stream.target_audio_channels(None);
                if let Some(value) = apply_numeric_condition(current, value, condition.condition) {
                    stream.set_option("audiochannels", value.to_string());
                }
            }
            _ => {}
        }
    }
}

fn apply_numeric_condition(
    current: Option<i32>,
    value: i32,
    condition: ProfileConditionType,
) -> Option<i32> {
    match condition {
        ProfileConditionType::Equals => Some(value),
        ProfileConditionType::LessThanEqual => Some(match current {
            Some(current) => current.min(value),
            None => value,
        }),
        ProfileConditionType::GreaterThanEqual => Some(match current {
            Some(current) => current.max(value),
            None => value,
        }),
        ProfileConditionType::NotEquals | ProfileConditionType::EqualsAny => current,
    }
}

fn normalize_container(
    input: Option<&str>,
    profile: &DeviceProfile,
    profile_type: DlnaProfileType,
    play_profile: Option<&DirectPlayProfile>,
) -> Option<String> {
    let input = input?;
    if !input.contains(',') {
        return Some(input.to_owned());
    }
    for format in ContainerHelper::split(Some(input)) {
        let supported = play_profile.map_or_else(
            || {
                profile.direct_play_profiles.iter().any(|candidate| {
                    candidate.profile_type == profile_type
                        && candidate.supports_container(Some(format))
                })
            },
            |candidate| {
                candidate.profile_type == profile_type && candidate.supports_container(Some(format))
            },
        );
        if supported {
            return Some(format.to_owned());
        }
    }
    Some(input.to_owned())
}

fn audio_stream_rank(stream: &StreamInfo, max_bitrate: i64, index: usize) -> AudioStreamRank {
    let source = stream.media_source.as_ref();
    AudioStreamRank {
        direct_file: !(stream.play_method == PlayMethod::DirectPlay
            && source.is_some_and(|source| source.protocol == MediaProtocol::File)),
        direct: !matches!(
            stream.play_method,
            PlayMethod::DirectPlay | PlayMethod::DirectStream
        ),
        file: !source.is_some_and(|source| source.protocol == MediaProtocol::File),
        bitrate_distance: if max_bitrate > 0 {
            source
                .and_then(|source| source.bitrate)
                .map_or(0, |bitrate| (i64::from(bitrate) - max_bitrate).abs())
        } else {
            0
        },
        index,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AudioStreamRank {
    direct_file: bool,
    direct: bool,
    file: bool,
    bitrate_distance: i64,
    index: usize,
}

impl Ord for AudioStreamRank {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.direct_file,
            self.direct,
            self.file,
            self.bitrate_distance,
            self.index,
        )
            .cmp(&(
                other.direct_file,
                other.direct,
                other.file,
                other.bitrate_distance,
                other.index,
            ))
    }
}

impl PartialOrd for AudioStreamRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
