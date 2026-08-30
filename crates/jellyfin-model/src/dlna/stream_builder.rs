use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ContainerHelper, DlnaProfileType, EncodingContext, MediaProtocol, MediaSourceInfo,
    MediaStreamProtocol, PlayMethod, ProfileCondition, ProfileConditionType, ProfileConditionValue,
    StreamInfo, TranscodeReason, TranscodeSeekInfo,
};
use crate::{MediaStream, MediaStreamType, SubtitleDeliveryMethod, VideoRangeType};

const HLS_AUDIO_CODECS_TS: &[&str] = &["aac", "ac3", "eac3", "mp3"];
const HLS_AUDIO_CODECS_MP4: &[&str] = &[
    "aac", "ac3", "eac3", "mp3", "alac", "flac", "opus", "dts", "truehd",
];

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct DirectPlayProfile {
    pub container: String,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    #[serde(rename = "Type")]
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

    #[must_use]
    pub fn supports_video_codec(&self, codec: Option<&str>) -> bool {
        self.profile_type == DlnaProfileType::Video
            && ContainerHelper::contains_container(self.video_codec.as_deref(), codec)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum CodecType {
    #[default]
    Video = 0,
    VideoAudio = 1,
    Audio = 2,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CodecProfile {
    #[serde(rename = "Type")]
    pub profile_type: CodecType,
    pub conditions: Vec<ProfileCondition>,
    pub apply_conditions: Vec<ProfileCondition>,
    pub codec: Option<String>,
    pub container: Option<String>,
    pub sub_container: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SubtitleProfile {
    pub format: String,
    pub method: SubtitleDeliveryMethod,
    pub language: Option<String>,
    pub container: Option<String>,
}

impl SubtitleProfile {
    fn supports_language(&self, language: Option<&str>) -> bool {
        let language = language.filter(|value| !value.is_empty()).unwrap_or("und");
        self.language.as_deref().is_none_or(|supported| {
            supported.is_empty()
                || ContainerHelper::contains_container(Some(supported), Some(language))
        })
    }
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

    fn contains_video_codec(
        &self,
        codec: Option<&str>,
        container: Option<&str>,
        use_sub_container: bool,
    ) -> bool {
        let profile_container = if use_sub_container
            && self
                .container
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("hls"))
        {
            self.sub_container.as_deref()
        } else {
            self.container.as_deref()
        };
        ContainerHelper::contains_container(profile_container, container)
            && ContainerHelper::contains_container_span_with_polarity(
                self.codec.as_deref(),
                false,
                codec.unwrap_or_default(),
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TranscodingProfile {
    pub container: String,
    #[serde(rename = "Type")]
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
    #[serde(deserialize_with = "deserialize_i32_from_number_or_string")]
    pub min_segments: i32,
    pub segment_length: i32,
    pub conditions: Vec<ProfileCondition>,
    pub enable_audio_vbr_encoding: bool,
}

fn deserialize_i32_from_number_or_string<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrString {
        Number(i32),
        String(String),
    }

    match NumberOrString::deserialize(deserializer)? {
        NumberOrString::Number(value) => Ok(value),
        NumberOrString::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
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
    pub subtitle_profiles: Vec<SubtitleProfile>,
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
            subtitle_profiles: Vec::new(),
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
    pub allow_video_stream_copy: bool,
    pub always_burn_in_subtitle_when_transcoding: bool,
    pub item_id: Uuid,
    pub media_sources: Vec<MediaSourceInfo>,
    pub profile: DeviceProfile,
    pub media_source_id: Option<String>,
    pub device_id: Option<String>,
    pub max_audio_channels: Option<i32>,
    pub max_bitrate: Option<i32>,
    pub context: EncodingContext,
    pub audio_transcoding_bitrate: Option<i32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

impl Default for MediaOptions {
    fn default() -> Self {
        Self {
            enable_direct_play: true,
            enable_direct_stream: true,
            force_direct_play: false,
            force_direct_stream: false,
            allow_audio_stream_copy: false,
            allow_video_stream_copy: false,
            always_burn_in_subtitle_when_transcoding: false,
            item_id: Uuid::nil(),
            media_sources: Vec::new(),
            profile: DeviceProfile::default(),
            media_source_id: None,
            device_id: None,
            max_audio_channels: None,
            max_bitrate: None,
            context: EncodingContext::default(),
            audio_transcoding_bitrate: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
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
    MissingMediaSourceId,
    MissingAudioStream,
}

impl std::fmt::Display for StreamBuilderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDeviceId => {
                formatter.write_str("device id is required for an empty item id")
            }
            Self::MissingMediaSourceId => formatter
                .write_str("media source id is required when an explicit stream is requested"),
            Self::MissingAudioStream => formatter.write_str("media source has no audio stream"),
        }
    }
}

impl std::error::Error for StreamBuilderError {}

/// Pure DLNA stream selection; codec capability is supplied by the caller.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamBuilder {
    encodable_audio_codecs: Option<Vec<String>>,
    subtitle_extraction_supported: Option<bool>,
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
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn with_subtitle_extraction_support(mut self, supported: bool) -> Self {
        self.subtitle_extraction_supported = Some(supported);
        self
    }

    #[must_use]
    pub fn get_subtitle_profile(
        &self,
        subtitle: &MediaStream,
        profiles: &[SubtitleProfile],
        method: PlayMethod,
        output_container: Option<&str>,
        protocol: Option<MediaStreamProtocol>,
    ) -> SubtitleProfile {
        select_subtitle_profile(
            subtitle,
            profiles,
            method,
            output_container,
            protocol,
            self.subtitle_extraction_supported
                .unwrap_or(subtitle.supports_external_stream),
        )
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

    pub fn get_optimal_video_stream(
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
        if (options.audio_stream_index.is_some() || options.subtitle_stream_index.is_some())
            && options
                .media_source_id
                .as_deref()
                .is_none_or(|id| id.is_empty())
        {
            return Err(StreamBuilderError::MissingMediaSourceId);
        }

        let max_bitrate = i64::from(options.max_bitrate(false).unwrap_or_default());
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
            let mut stream = self.build_video_stream(source, options);
            stream.device_id.clone_from(&options.device_id);
            stream.device_profile_id = options.profile.id.map(|id| id.simple().to_string());
            streams.push((index, stream));
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

    fn build_video_stream(&self, source: &MediaSourceInfo, options: &MediaOptions) -> StreamInfo {
        let mut stream = StreamInfo::new(options.item_id, DlnaProfileType::Video);
        stream.media_source = Some(source.clone());
        stream.run_time_ticks = source.run_time_ticks;
        stream.context = options.context;
        stream.always_burn_in_subtitle_when_transcoding =
            options.always_burn_in_subtitle_when_transcoding;
        stream.subtitle_stream_index = options
            .subtitle_stream_index
            .or(source.default_subtitle_stream_index);

        let video = source.video_stream();
        let audio = source.default_audio_stream(
            options
                .audio_stream_index
                .or(source.default_audio_stream_index),
        );
        stream.audio_stream_index = audio.map(|audio| audio.index);
        let subtitle = stream
            .subtitle_stream_index
            .and_then(|index| source.media_stream(MediaStreamType::Subtitle, index));

        let bitrate_exceeded =
            bitrate_limit_exceeded(source, options.max_bitrate(false).unwrap_or_default());
        let direct_play_eligible = options.enable_direct_play
            && (options.force_direct_play || !bitrate_exceeded)
            && !matches!(
                source.video_type,
                Some(super::VideoType::Dvd | super::VideoType::BluRay)
            );
        let direct_stream_eligible =
            options.enable_direct_stream && (options.force_direct_stream || !bitrate_exceeded);
        let mut reasons = if bitrate_exceeded {
            TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT
        } else {
            TranscodeReason::NONE
        };

        let direct = if options.force_direct_play {
            Some(VideoDecision {
                profile: None,
                method: Some(PlayMethod::DirectPlay),
                audio_index: audio.map(|audio| audio.index),
                reasons: TranscodeReason::NONE,
            })
        } else if options.force_direct_stream {
            Some(VideoDecision {
                profile: None,
                method: Some(PlayMethod::DirectStream),
                audio_index: audio.map(|audio| audio.index),
                reasons: TranscodeReason::NONE,
            })
        } else if direct_play_eligible || direct_stream_eligible {
            Some(video_direct_play_profile(
                self,
                source,
                video,
                audio,
                subtitle,
                options,
                DirectPlayEligibility {
                    direct_play: direct_play_eligible,
                    direct_stream: direct_stream_eligible,
                },
            ))
        } else {
            None
        };

        if let Some(direct) = direct {
            reasons |= direct.reasons;
            if let Some(method) = direct.method {
                stream.play_method = method;
                stream.container = normalize_container(
                    source.container.as_deref(),
                    &options.profile,
                    DlnaProfileType::Video,
                    direct.profile,
                );
                stream.video_codecs = video
                    .and_then(|video| video.codec.clone())
                    .into_iter()
                    .collect();
                stream.audio_stream_index = direct.audio_index.or(stream.audio_stream_index);
                if method == PlayMethod::DirectPlay {
                    stream.audio_codecs = stream
                        .audio_stream_index
                        .and_then(|index| source.media_stream(MediaStreamType::Audio, index))
                        .and_then(|audio| audio.codec.clone())
                        .into_iter()
                        .collect();
                    stream.sub_protocol = MediaStreamProtocol::Http;
                } else {
                    stream.audio_codecs = direct
                        .profile
                        .and_then(|profile| profile.audio_codec.as_deref())
                        .map(|value| ContainerHelper::split(Some(value)))
                        .unwrap_or_default()
                        .into_iter()
                        .map(str::to_owned)
                        .collect();
                    if let Some(profile) = direct.profile {
                        stream.container = normalize_container(
                            source.container.as_deref(),
                            &options.profile,
                            DlnaProfileType::Video,
                            Some(profile),
                        );
                    }
                    stream.sub_protocol = MediaStreamProtocol::Http;
                    build_video_targets(
                        &mut stream,
                        source,
                        video,
                        audio,
                        direct.profile.map(|profile| profile.container.as_str()),
                        direct
                            .profile
                            .and_then(|profile| profile.video_codec.as_deref()),
                        direct
                            .profile
                            .and_then(|profile| profile.audio_codec.as_deref()),
                        options,
                    );
                }
                if let Some(subtitle) = subtitle {
                    let profile = self.get_subtitle_profile(
                        subtitle,
                        &options.profile.subtitle_profiles,
                        method,
                        direct.profile.map(|profile| profile.container.as_str()),
                        None,
                    );
                    stream.subtitle_delivery_method = profile.method;
                    stream.subtitle_format = Some(profile.format);
                }
                stream.transcode_reasons = reasons;
                return stream;
            }
        }

        stream.transcode_reasons = reasons;
        if let Some(profile) = choose_video_transcoding_profile(source, video, audio, options) {
            apply_transcoding_profile(&mut stream, profile);
            build_video_targets(
                &mut stream,
                source,
                video,
                audio,
                Some(&profile.container),
                Some(&profile.video_codec),
                Some(&profile.audio_codec),
                options,
            );
            stream.play_method = PlayMethod::Transcode;
            if let Some(subtitle) = subtitle {
                let subtitle_profile = self.get_subtitle_profile(
                    subtitle,
                    &options.profile.subtitle_profiles,
                    PlayMethod::Transcode,
                    Some(&profile.container),
                    Some(profile.protocol),
                );
                stream.subtitle_delivery_method = subtitle_profile.method;
                stream.subtitle_format = Some(subtitle_profile.format.clone());
                stream.subtitle_codecs = vec![subtitle_profile.format];
            }
            if intersects(
                stream.transcode_reasons,
                video_reasons() | TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT,
            ) {
                apply_general_transcoding_conditions(&mut stream, &profile.conditions, None);
            }
        }
        stream
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
struct VideoDecision<'a> {
    profile: Option<&'a DirectPlayProfile>,
    method: Option<PlayMethod>,
    audio_index: Option<i32>,
    reasons: TranscodeReason,
}

fn video_direct_play_profile<'a>(
    builder: &StreamBuilder,
    source: &MediaSourceInfo,
    video: Option<&MediaStream>,
    audio: Option<&MediaStream>,
    subtitle: Option<&MediaStream>,
    options: &'a MediaOptions,
    eligibility: DirectPlayEligibility,
) -> VideoDecision<'a> {
    let container_reasons = compatibility_container(&options.profile, source, video);
    let video_profile_reasons = video.map_or(TranscodeReason::NONE, |video| {
        compatibility_video_codec(&options.profile, source, video)
    });
    let subtitle_reasons = subtitle.map_or(TranscodeReason::NONE, |subtitle| {
        let profile = builder.get_subtitle_profile(
            subtitle,
            &options.profile.subtitle_profiles,
            PlayMethod::DirectPlay,
            source.container.as_deref(),
            None,
        );
        if matches!(
            profile.method,
            SubtitleDeliveryMethod::Drop
                | SubtitleDeliveryMethod::External
                | SubtitleDeliveryMethod::Embed
        ) {
            TranscodeReason::NONE
        } else {
            TranscodeReason::SUBTITLE_CODEC_NOT_SUPPORTED
        }
    });

    let candidates = source
        .media_streams
        .iter()
        .filter(|stream| stream.stream_type == MediaStreamType::Audio)
        .collect::<Vec<_>>();
    let candidates = if let Some(audio) = audio {
        if options.audio_stream_index.is_some() {
            vec![audio]
        } else if audio.is_default {
            candidates
                .into_iter()
                .filter(|candidate| candidate.is_default)
                .collect()
        } else {
            candidates
        }
    } else {
        Vec::new()
    };

    let mut analyzed = Vec::new();
    let mut any_container_supported = false;
    for (order, profile) in options
        .profile
        .direct_play_profiles
        .iter()
        .filter(|profile| profile.profile_type == DlnaProfileType::Video)
        .enumerate()
    {
        let mut reasons = TranscodeReason::NONE;
        if profile.supports_container(source.container.as_deref()) {
            any_container_supported = true;
        } else {
            reasons |= TranscodeReason::CONTAINER_NOT_SUPPORTED;
        }
        if !profile.supports_video_codec(video.and_then(|video| video.codec.as_deref())) {
            reasons |= TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED;
        }
        let selected_audio = if candidates.is_empty() {
            None
        } else {
            candidates
                .iter()
                .copied()
                .find(|audio| profile.supports_audio_codec(audio.codec.as_deref()))
        };
        if !candidates.is_empty() && selected_audio.is_none() {
            reasons |= TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED;
        }
        reasons |= container_reasons | subtitle_reasons;
        if !intersects(reasons, TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED) {
            reasons |= video_profile_reasons;
        }
        if !intersects(reasons, TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED)
            && let Some(audio) = selected_audio
        {
            reasons |= compatibility_video_audio(
                &options.profile,
                source,
                audio,
                is_secondary_audio(source, audio).unwrap_or(false),
            );
            if audio.is_external {
                reasons |= TranscodeReason::AUDIO_IS_EXTERNAL;
            }
        }

        let direct_stream_failures = reasons.bits() & !direct_stream_reasons().bits();
        let method = if reasons.is_empty() && eligibility.direct_play && source.supports_direct_play
        {
            Some(PlayMethod::DirectPlay)
        } else if direct_stream_failures == 0
            && eligibility.direct_stream
            && source.supports_direct_stream
        {
            Some(PlayMethod::DirectStream)
        } else {
            None
        };
        analyzed.push(VideoCandidate {
            decision: VideoDecision {
                profile: Some(profile),
                method,
                audio_index: selected_audio.map(|audio| audio.index),
                reasons,
            },
            order,
            rank: failure_rank(reasons),
        });
    }

    analyzed.sort_by(|left, right| {
        method_rank(right.decision.method)
            .cmp(&method_rank(left.decision.method))
            .then_with(|| right.rank.cmp(&left.rank))
            .then_with(|| left.order.cmp(&right.order))
    });
    if let Some(candidate) = analyzed
        .iter()
        .find(|candidate| candidate.decision.method.is_some())
    {
        return candidate.decision;
    }
    let reasons = analyzed
        .iter()
        .find(|candidate| {
            !any_container_supported
                || !intersects(
                    candidate.decision.reasons,
                    TranscodeReason::CONTAINER_NOT_SUPPORTED,
                )
        })
        .map_or(TranscodeReason::DIRECT_PLAY_ERROR, |candidate| {
            if candidate.decision.reasons.is_empty() {
                TranscodeReason::DIRECT_PLAY_ERROR
            } else {
                candidate.decision.reasons
            }
        });
    VideoDecision {
        profile: None,
        method: None,
        audio_index: None,
        reasons,
    }
}

#[derive(Clone, Copy)]
struct DirectPlayEligibility {
    direct_play: bool,
    direct_stream: bool,
}

struct VideoCandidate<'a> {
    decision: VideoDecision<'a>,
    order: usize,
    rank: u8,
}

const fn method_rank(method: Option<PlayMethod>) -> u8 {
    match method {
        Some(PlayMethod::DirectPlay) => 2,
        Some(PlayMethod::DirectStream) => 1,
        _ => 0,
    }
}

fn failure_rank(reasons: TranscodeReason) -> u8 {
    let rankings = [
        TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED,
        video_codec_reasons(),
        TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED,
        audio_codec_reasons(),
        container_reasons(),
    ];
    rankings
        .iter()
        .position(|flag| intersects(reasons, *flag))
        .map_or(6, |index| index as u8 + 1)
}

fn compatibility_container(
    profile: &DeviceProfile,
    source: &MediaSourceInfo,
    video: Option<&MediaStream>,
) -> TranscodeReason {
    profile
        .container_profiles
        .iter()
        .filter(|container_profile| {
            container_profile.profile_type == DlnaProfileType::Video
                && container_profile
                    .contains_container(source.container.as_deref().unwrap_or_default(), false)
        })
        .flat_map(|container_profile| &container_profile.conditions)
        .filter(|condition| !video_condition_satisfied(condition, source, video))
        .fold(TranscodeReason::NONE, |reasons, condition| {
            reasons | transcode_reason_for_failed_condition(condition.property)
        })
}

fn compatibility_video_codec(
    profile: &DeviceProfile,
    source: &MediaSourceInfo,
    video: &MediaStream,
) -> TranscodeReason {
    profile
        .codec_profiles
        .iter()
        .filter(|codec_profile| {
            codec_profile.profile_type == CodecType::Video
                && codec_profile.contains_video_codec(
                    video.codec.as_deref(),
                    source.container.as_deref(),
                    false,
                )
                && codec_profile
                    .apply_conditions
                    .iter()
                    .all(|condition| video_condition_satisfied(condition, source, Some(video)))
        })
        .flat_map(|codec_profile| &codec_profile.conditions)
        .filter(|condition| !video_condition_satisfied(condition, source, Some(video)))
        .fold(TranscodeReason::NONE, |reasons, condition| {
            reasons | transcode_reason_for_failed_condition(condition.property)
        })
}

fn compatibility_video_audio(
    profile: &DeviceProfile,
    source: &MediaSourceInfo,
    audio: &MediaStream,
    secondary: bool,
) -> TranscodeReason {
    profile
        .codec_profiles
        .iter()
        .filter(|codec_profile| {
            codec_profile.profile_type == CodecType::VideoAudio
                && codec_profile
                    .contains_audio_codec(audio.codec.as_deref(), source.container.as_deref())
                && codec_profile
                    .apply_conditions
                    .iter()
                    .all(|condition| video_audio_condition_satisfied(condition, audio, secondary))
        })
        .flat_map(|codec_profile| &codec_profile.conditions)
        .filter(|condition| !video_audio_condition_satisfied(condition, audio, secondary))
        .fold(TranscodeReason::NONE, |reasons, condition| {
            reasons | transcode_reason_for_failed_condition(condition.property)
        })
}

fn video_audio_condition_satisfied(
    condition: &ProfileCondition,
    audio: &MediaStream,
    secondary: bool,
) -> bool {
    match condition.property {
        ProfileConditionValue::AudioProfile => {
            string_condition_satisfied(condition, audio.profile.as_deref())
        }
        ProfileConditionValue::IsSecondaryAudio => {
            bool_condition_satisfied(condition, Some(secondary))
        }
        _ => audio_condition_satisfied(condition, audio),
    }
}

fn video_condition_satisfied(
    condition: &ProfileCondition,
    source: &MediaSourceInfo,
    video: Option<&MediaStream>,
) -> bool {
    match condition.property {
        ProfileConditionValue::Width => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.width))
        }
        ProfileConditionValue::Height => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.height))
        }
        ProfileConditionValue::VideoBitDepth => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.bit_depth))
        }
        ProfileConditionValue::VideoBitrate => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.bit_rate))
        }
        ProfileConditionValue::VideoFramerate => float_condition_satisfied(
            condition,
            video
                .and_then(MediaStream::reference_frame_rate)
                .map(f64::from),
        ),
        ProfileConditionValue::VideoLevel => {
            float_condition_satisfied(condition, video.and_then(|stream| stream.level))
        }
        ProfileConditionValue::VideoProfile => string_condition_satisfied(
            condition,
            video.and_then(|stream| stream.profile.as_deref()),
        ),
        ProfileConditionValue::VideoRangeType => {
            let value = video
                .map(|stream| stream.video_range_type)
                .filter(|value| *value != VideoRangeType::Unknown)
                .map(video_range_type_name);
            string_condition_satisfied(condition, value)
        }
        ProfileConditionValue::VideoCodecTag => string_condition_satisfied(
            condition,
            video.and_then(|stream| stream.codec_tag.as_deref()),
        ),
        ProfileConditionValue::IsAnamorphic => {
            bool_condition_satisfied(condition, video.and_then(|stream| stream.is_anamorphic))
        }
        ProfileConditionValue::IsInterlaced => {
            bool_condition_satisfied(condition, video.map(|stream| stream.is_interlaced))
        }
        ProfileConditionValue::IsAvc => bool_condition_satisfied(
            condition,
            video.map(|stream| {
                stream
                    .codec
                    .as_deref()
                    .is_some_and(|codec| codec.eq_ignore_ascii_case("h264"))
            }),
        ),
        ProfileConditionValue::RefFrames => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.ref_frames))
        }
        ProfileConditionValue::PacketLength => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.packet_length))
        }
        ProfileConditionValue::VideoRotation => {
            int_condition_satisfied(condition, video.and_then(|stream| stream.rotation))
        }
        ProfileConditionValue::NumStreams => {
            int_condition_satisfied(condition, Some(source.media_streams.len() as i32))
        }
        ProfileConditionValue::NumAudioStreams => int_condition_satisfied(
            condition,
            Some(
                source
                    .media_streams
                    .iter()
                    .filter(|stream| stream.stream_type == MediaStreamType::Audio)
                    .count() as i32,
            ),
        ),
        ProfileConditionValue::NumVideoStreams => int_condition_satisfied(
            condition,
            Some(
                source
                    .media_streams
                    .iter()
                    .filter(|stream| stream.stream_type == MediaStreamType::Video)
                    .count() as i32,
            ),
        ),
        _ => true,
    }
}

fn int_condition_satisfied(condition: &ProfileCondition, current: Option<i32>) -> bool {
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
    compare_ordered(condition.condition, current, expected)
}

fn float_condition_satisfied(condition: &ProfileCondition, current: Option<f64>) -> bool {
    let Some(current) = current else {
        return !condition.is_required;
    };
    if condition.condition == ProfileConditionType::EqualsAny {
        return condition
            .value
            .split('|')
            .filter_map(|value| value.parse::<f64>().ok())
            .any(|value| value == current);
    }
    let Ok(expected) = condition.value.parse::<f64>() else {
        return false;
    };
    compare_ordered(condition.condition, current, expected)
}

fn compare_ordered<T: PartialOrd + PartialEq>(
    condition: ProfileConditionType,
    current: T,
    expected: T,
) -> bool {
    match condition {
        ProfileConditionType::Equals => current == expected,
        ProfileConditionType::NotEquals => current != expected,
        ProfileConditionType::LessThanEqual => current <= expected,
        ProfileConditionType::GreaterThanEqual => current >= expected,
        ProfileConditionType::EqualsAny => false,
    }
}

fn string_condition_satisfied(condition: &ProfileCondition, current: Option<&str>) -> bool {
    let Some(current) = current.filter(|value| !value.is_empty()) else {
        return !condition.is_required;
    };
    match condition.condition {
        ProfileConditionType::EqualsAny => condition
            .value
            .split('|')
            .any(|value| value.eq_ignore_ascii_case(current)),
        ProfileConditionType::Equals => condition.value.eq_ignore_ascii_case(current),
        ProfileConditionType::NotEquals => !condition.value.eq_ignore_ascii_case(current),
        _ => false,
    }
}

fn bool_condition_satisfied(condition: &ProfileCondition, current: Option<bool>) -> bool {
    let Some(current) = current else {
        return !condition.is_required;
    };
    let Ok(expected) = condition.value.parse::<bool>() else {
        return false;
    };
    match condition.condition {
        ProfileConditionType::Equals => current == expected,
        ProfileConditionType::NotEquals => current != expected,
        _ => false,
    }
}

const fn video_range_type_name(value: VideoRangeType) -> &'static str {
    match value {
        VideoRangeType::Unknown => "Unknown",
        VideoRangeType::Sdr => "SDR",
        VideoRangeType::Hdr10 => "HDR10",
        VideoRangeType::Hlg => "HLG",
        VideoRangeType::Dovi => "DOVI",
        VideoRangeType::DoviWithHdr10 => "DOVIWithHDR10",
        VideoRangeType::DoviWithHlg => "DOVIWithHLG",
        VideoRangeType::DoviWithSdr => "DOVIWithSDR",
        VideoRangeType::DoviWithEl => "DOVIWithEL",
        VideoRangeType::DoviWithHdr10Plus => "DOVIWithHDR10Plus",
        VideoRangeType::DoviWithElHdr10Plus => "DOVIWithELHDR10Plus",
        VideoRangeType::DoviInvalid => "DOVIInvalid",
        VideoRangeType::Hdr10Plus => "HDR10Plus",
    }
}

fn choose_video_transcoding_profile<'a>(
    source: &MediaSourceInfo,
    video: Option<&MediaStream>,
    audio: Option<&MediaStream>,
    options: &'a MediaOptions,
) -> Option<&'a TranscodingProfile> {
    if !(source.supports_transcoding || source.supports_direct_stream) {
        return None;
    }
    let mut profiles = options
        .profile
        .transcoding_profiles
        .iter()
        .filter(|profile| {
            profile.profile_type == DlnaProfileType::Video
                && profile.context == options.context
                && (!source.use_most_compatible_transcoding_profile
                    || profile.container.eq_ignore_ascii_case("ts"))
        })
        .map(|profile| {
            let video_rank = if let Some(video) = video {
                if options.allow_video_stream_copy
                    && ContainerHelper::contains_container(
                        Some(&profile.video_codec),
                        video.codec.as_deref(),
                    )
                {
                    if compatibility_video_codec(&options.profile, source, video).is_empty() {
                        1
                    } else {
                        2
                    }
                } else {
                    3
                }
            } else {
                3
            };
            let mut audio_rank = 3;
            if let Some(audio) = audio.filter(|_| options.allow_audio_stream_copy) {
                for codec in ContainerHelper::split(Some(&profile.audio_codec)) {
                    let failures =
                        compatibility_video_audio(&options.profile, source, audio, false);
                    if failures.is_empty() {
                        audio_rank = audio_rank.min(
                            if audio
                                .codec
                                .as_deref()
                                .is_some_and(|input| input.eq_ignore_ascii_case(codec))
                            {
                                1
                            } else {
                                2
                            },
                        );
                    }
                }
            }
            ((video_rank, audio_rank), profile)
        })
        .collect::<Vec<_>>();
    profiles.sort_by_key(|(rank, _)| *rank);
    profiles.into_iter().next().map(|(_, profile)| profile)
}

#[allow(clippy::too_many_arguments)]
fn build_video_targets(
    stream: &mut StreamInfo,
    source: &MediaSourceInfo,
    video: Option<&MediaStream>,
    audio: Option<&MediaStream>,
    container: Option<&str>,
    video_codec: Option<&str>,
    audio_codec: Option<&str>,
    options: &MediaOptions,
) {
    let mut video_codecs = ContainerHelper::split(video_codec)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if video_codecs.is_empty()
        && let Some(codec) = video.and_then(|video| video.codec.clone())
    {
        video_codecs.push(codec);
    }
    if stream.sub_protocol == MediaStreamProtocol::Hls {
        video_codecs.retain(|codec| ["h264", "hevc", "vp9", "av1"].contains(&codec.as_str()));
    }
    stream.video_codecs = video_codecs;
    if let Some(video) = video
        && !codec_list_contains(&stream.video_codecs, video.codec.as_deref())
    {
        stream.transcode_reasons |= TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED;
    }
    stream.max_framerate = video.and_then(MediaStream::reference_frame_rate);
    if let Some(video) = video {
        let qualifier = video.codec.as_deref();
        if let Some(level) = video.level {
            stream.set_qualified_option(qualifier, "level", level.to_string());
        }
        if let Some(bit_depth) = video.bit_depth {
            stream.set_qualified_option(qualifier, "videobitdepth", bit_depth.to_string());
        }
        if let Some(profile) = video.profile.as_deref().filter(|value| !value.is_empty()) {
            stream.set_qualified_option(qualifier, "profile", profile.to_ascii_lowercase());
        }
    }

    let mut audio_codecs = ContainerHelper::split(audio_codec)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if audio_codecs.is_empty()
        && let Some(codec) = audio.and_then(|audio| audio.codec.clone())
    {
        audio_codecs.push(codec);
    }
    if stream.sub_protocol == MediaStreamProtocol::Hls {
        let supported = if stream
            .container
            .as_deref()
            .is_some_and(|container| container.eq_ignore_ascii_case("mp4"))
        {
            HLS_AUDIO_CODECS_MP4
        } else {
            HLS_AUDIO_CODECS_TS
        };
        audio_codecs.retain(|codec| supported.contains(&codec.as_str()));
    }

    let selected_audio = if options.audio_stream_index.is_some() {
        audio.filter(|candidate| codec_list_contains(&audio_codecs, candidate.codec.as_deref()))
    } else {
        source
            .media_streams
            .iter()
            .filter(|candidate| candidate.stream_type == MediaStreamType::Audio)
            .filter(|candidate| {
                audio.is_none_or(|selected| !selected.is_default || candidate.is_default)
            })
            .find(|candidate| codec_list_contains(&audio_codecs, candidate.codec.as_deref()))
    };
    let channels_exceed = selected_audio.is_some_and(|audio| {
        audio.channels.unwrap_or_default()
            > stream.transcoding_max_audio_channels.unwrap_or(i32::MAX)
    });
    let audio_failures = selected_audio.map_or(TranscodeReason::NONE, |audio| {
        compatibility_video_audio(
            &options.profile,
            source,
            audio,
            is_secondary_audio(source, audio).unwrap_or(false),
        )
    });
    stream.transcode_reasons |= audio_failures;
    if audio.is_some() && selected_audio.is_none() {
        stream.transcode_reasons |= TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED;
    }
    if channels_exceed {
        stream.transcode_reasons |= TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED;
    }
    let can_copy_audio = selected_audio.is_some()
        && !channels_exceed
        && audio_failures.is_empty()
        && !intersects(
            stream.transcode_reasons,
            TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT,
        );
    stream.audio_codecs = audio_codecs;
    let output_audio = if can_copy_audio {
        selected_audio
    } else {
        audio
    };
    if let Some(output_audio) = selected_audio.filter(|_| can_copy_audio) {
        stream.audio_stream_index = Some(output_audio.index);
        stream.audio_codecs = output_audio.codec.clone().into_iter().collect();
        stream.audio_sample_rate = output_audio.sample_rate;
        stream.set_option(
            "audiochannels",
            output_audio
                .channels
                .map_or_else(String::new, |value| value.to_string()),
        );
    }

    let use_sub_container = stream.sub_protocol == MediaStreamProtocol::Hls;
    let target_video_codecs = stream.video_codecs.clone();
    for codec_profile in options
        .profile
        .codec_profiles
        .iter()
        .filter(|profile| {
            profile.profile_type == CodecType::Video
                && target_video_codecs.iter().any(|codec| {
                    profile.contains_video_codec(Some(codec), container, use_sub_container)
                })
                && profile
                    .apply_conditions
                    .iter()
                    .all(|condition| video_condition_satisfied(condition, source, video))
        })
        .rev()
    {
        for codec in &target_video_codecs {
            if codec_profile.contains_video_codec(Some(codec), container, use_sub_container) {
                apply_general_transcoding_conditions(
                    stream,
                    &codec_profile.conditions,
                    Some(codec),
                );
            }
        }
    }

    stream.global_max_audio_channels = if channels_exceed {
        stream.transcoding_max_audio_channels
    } else {
        options.max_audio_channels
    };
    let audio_bitrate = get_audio_bitrate(
        i64::from(options.max_bitrate(true).unwrap_or_default()),
        &stream.audio_codecs,
        output_audio,
        stream,
    );
    stream.audio_bitrate = Some(
        stream
            .audio_bitrate
            .unwrap_or(audio_bitrate)
            .min(audio_bitrate),
    );

    let target_audio_codecs = stream.audio_codecs.clone();
    for codec_profile in options
        .profile
        .codec_profiles
        .iter()
        .filter(|profile| {
            profile.profile_type == CodecType::VideoAudio
                && target_audio_codecs
                    .iter()
                    .any(|codec| profile.contains_audio_codec(Some(codec), container))
        })
        .rev()
    {
        for codec in &target_audio_codecs {
            if codec_profile.contains_audio_codec(Some(codec), container) {
                apply_general_transcoding_conditions(
                    stream,
                    &codec_profile.conditions,
                    Some(codec),
                );
                break;
            }
        }
    }

    if let Some(max_bitrate) = options.max_bitrate(false) {
        let available = max_bitrate - stream.audio_bitrate.unwrap_or_default();
        let current = stream.video_bitrate.unwrap_or(available);
        stream.video_bitrate = Some(available.min(current).max(64_000));
    }
}

fn codec_list_contains(codecs: &[String], codec: Option<&str>) -> bool {
    codec.is_some_and(|codec| codecs.iter().any(|item| item.eq_ignore_ascii_case(codec)))
}

fn get_audio_bitrate(
    max_total: i64,
    codecs: &[String],
    audio: Option<&MediaStream>,
    stream: &StreamInfo,
) -> i32 {
    let codec = codecs.first().map(String::as_str);
    let target_channels = stream.target_audio_channels(codec);
    let mut bitrate = audio.map_or(192_000, |audio| {
        if target_channels
            .zip(audio.channels)
            .is_some_and(|(target, input)| input > target)
            || (target_channels
                .zip(audio.channels)
                .is_some_and(|(target, input)| input <= target)
                && !codec_list_contains(codecs, audio.codec.as_deref()))
        {
            default_audio_bitrate(codec, target_channels.or(audio.channels))
        } else {
            audio
                .bit_rate
                .unwrap_or_else(|| default_audio_bitrate(codec, target_channels))
        }
    });
    if max_total > 0 {
        bitrate = bitrate.min(max_audio_bitrate_for_total(max_total));
    }
    if audio.is_some_and(|audio| {
        audio.channels == Some(1) && audio.bit_rate.unwrap_or_default() < 64_000
    }) {
        bitrate = bitrate.min(64_000);
    }
    bitrate
}

fn default_audio_bitrate(codec: Option<&str>, channels: Option<i32>) -> i32 {
    if codec.is_some_and(|codec| ["aac", "mp3", "ac3", "eac3"].contains(&codec)) {
        return if channels.unwrap_or_default() < 2 {
            128_000
        } else if channels.unwrap_or_default() >= 6 {
            640_000
        } else {
            384_000
        };
    }
    if codec.is_some_and(|codec| ["flac", "alac"].contains(&codec)) {
        return if channels.unwrap_or_default() < 2 {
            768_000
        } else if channels.unwrap_or_default() >= 6 {
            3_584_000
        } else {
            1_536_000
        };
    }
    192_000
}

const fn max_audio_bitrate_for_total(total: i64) -> i32 {
    match total {
        ..=640_000 => 128_000,
        640_001..=2_000_000 => 384_000,
        2_000_001..=3_000_000 => 448_000,
        3_000_001..=4_000_000 => 640_000,
        4_000_001..=5_000_000 => 768_000,
        5_000_001..=10_000_000 => 1_536_000,
        10_000_001..=15_000_000 => 2_304_000,
        15_000_001..=20_000_000 => 3_584_000,
        _ => 7_168_000,
    }
}

fn select_subtitle_profile(
    subtitle: &MediaStream,
    profiles: &[SubtitleProfile],
    method: PlayMethod,
    output_container: Option<&str>,
    protocol: Option<MediaStreamProtocol>,
    can_extract_subtitles: bool,
) -> SubtitleProfile {
    let can_embed = if subtitle.is_external {
        method == PlayMethod::Transcode
            && protocol != Some(MediaStreamProtocol::Hls)
            && subtitle_embed_supported(output_container)
    } else {
        method != PlayMethod::Transcode || protocol != Some(MediaStreamProtocol::Hls)
    };
    if can_embed {
        let eligible = |profile: &&SubtitleProfile| {
            profile.method == SubtitleDeliveryMethod::Embed
                && profile.supports_language(subtitle.language.as_deref())
                && ContainerHelper::contains_container(
                    profile.container.as_deref(),
                    output_container,
                )
                && (method != PlayMethod::Transcode || subtitle_embed_supported(output_container))
        };
        if let Some(profile) = profiles.iter().filter(eligible).find(|profile| {
            subtitle.is_text_subtitle_stream() == MediaStream::is_text_format(Some(&profile.format))
                && profile
                    .format
                    .eq_ignore_ascii_case(subtitle.codec.as_deref().unwrap_or_default())
        }) {
            return profile.clone();
        }
        if let Some(profile) = profiles
            .iter()
            .filter(eligible)
            .find(|profile| subtitle.supports_subtitle_conversion_to(&profile.format))
        {
            return profile.clone();
        }
    }

    for allow_conversion in [false, true] {
        if let Some(profile) = profiles.iter().find(|profile| {
            if !matches!(
                profile.method,
                SubtitleDeliveryMethod::External | SubtitleDeliveryMethod::Hls
            ) || (profile.method == SubtitleDeliveryMethod::Hls
                && method != PlayMethod::Transcode)
                || !profile.supports_language(subtitle.language.as_deref())
                || (!subtitle.is_external
                    && method == PlayMethod::Transcode
                    && !can_extract_subtitles
                    && !subtitle.is_pgs_subtitle_stream()
                    && !subtitle.is_vobsub_subtitle_stream())
            {
                return false;
            }
            let type_matches = match profile.method {
                SubtitleDeliveryMethod::External => {
                    subtitle.is_text_subtitle_stream()
                        == MediaStream::is_text_format(Some(&profile.format))
                }
                SubtitleDeliveryMethod::Hls => subtitle.is_text_subtitle_stream(),
                _ => false,
            };
            if !type_matches {
                return false;
            }
            let requires_conversion = !profile
                .format
                .eq_ignore_ascii_case(subtitle.codec.as_deref().unwrap_or_default());
            !requires_conversion
                || (allow_conversion
                    && subtitle.supports_external_stream
                    && subtitle.supports_subtitle_conversion_to(&profile.format))
        }) {
            return profile.clone();
        }
    }
    SubtitleProfile {
        format: subtitle.codec.clone().unwrap_or_default(),
        method: SubtitleDeliveryMethod::Encode,
        ..SubtitleProfile::default()
    }
}

fn subtitle_embed_supported(container: Option<&str>) -> bool {
    container.is_some_and(|container| {
        ContainerHelper::contains_container(Some("mkv,matroska"), Some(container))
    })
}

fn is_secondary_audio(source: &MediaSourceInfo, audio: &MediaStream) -> Option<bool> {
    if audio.is_external {
        return Some(false);
    }
    source
        .media_streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Audio && !stream.is_external)
        .map(|first| first.index != audio.index)
}

fn apply_general_transcoding_conditions(
    stream: &mut StreamInfo,
    conditions: &[ProfileCondition],
    qualifier: Option<&str>,
) {
    for condition in conditions {
        if condition.value.is_empty()
            || condition.condition == ProfileConditionType::GreaterThanEqual
        {
            continue;
        }
        match condition.property {
            ProfileConditionValue::AudioBitrate => {
                if let Ok(value) = condition.value.parse::<i32>() {
                    stream.audio_bitrate =
                        apply_numeric_condition(stream.audio_bitrate, value, condition.condition);
                }
            }
            ProfileConditionValue::AudioSampleRate => {
                if let Ok(value) = condition.value.parse::<i32>() {
                    stream.audio_sample_rate = apply_numeric_condition(
                        stream.audio_sample_rate,
                        value,
                        condition.condition,
                    );
                }
            }
            ProfileConditionValue::AudioChannels => {
                if let Ok(value) = condition.value.parse::<i32>()
                    && let Some(value) = apply_numeric_condition(
                        stream.target_audio_channels(qualifier),
                        value,
                        condition.condition,
                    )
                {
                    stream.set_qualified_option(qualifier, "audiochannels", value.to_string());
                }
            }
            ProfileConditionValue::Width => apply_stream_number(&mut stream.max_width, condition),
            ProfileConditionValue::Height => apply_stream_number(&mut stream.max_height, condition),
            ProfileConditionValue::VideoBitrate => {
                apply_stream_number(&mut stream.video_bitrate, condition)
            }
            ProfileConditionValue::VideoFramerate => {
                if let Ok(value) = condition.value.parse::<f32>() {
                    stream.max_framerate = match condition.condition {
                        ProfileConditionType::Equals => Some(value),
                        ProfileConditionType::LessThanEqual => {
                            Some(stream.max_framerate.unwrap_or(value).min(value))
                        }
                        _ => stream.max_framerate,
                    };
                }
            }
            ProfileConditionValue::VideoLevel
            | ProfileConditionValue::VideoBitDepth
            | ProfileConditionValue::VideoProfile
            | ProfileConditionValue::VideoRangeType => {
                let name = match condition.property {
                    ProfileConditionValue::VideoLevel => "level",
                    ProfileConditionValue::VideoBitDepth => "videobitdepth",
                    ProfileConditionValue::VideoProfile => "profile",
                    ProfileConditionValue::VideoRangeType => "rangetype",
                    _ => unreachable!(),
                };
                stream.set_qualified_option(qualifier, name, condition.value.clone());
            }
            _ => {}
        }
    }
}

fn apply_stream_number(target: &mut Option<i32>, condition: &ProfileCondition) {
    if let Ok(value) = condition.value.parse::<i32>() {
        *target = apply_numeric_condition(*target, value, condition.condition);
    }
}

const fn intersects(left: TranscodeReason, right: TranscodeReason) -> bool {
    left.bits() & right.bits() != 0
}

const fn container_reasons() -> TranscodeReason {
    TranscodeReason::from_bits_retain(
        TranscodeReason::CONTAINER_NOT_SUPPORTED.bits()
            | TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT.bits(),
    )
}

const fn audio_codec_reasons() -> TranscodeReason {
    TranscodeReason::from_bits_retain(
        TranscodeReason::AUDIO_BITRATE_NOT_SUPPORTED.bits()
            | TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED.bits()
            | TranscodeReason::AUDIO_PROFILE_NOT_SUPPORTED.bits()
            | TranscodeReason::AUDIO_SAMPLE_RATE_NOT_SUPPORTED.bits()
            | TranscodeReason::SECONDARY_AUDIO_NOT_SUPPORTED.bits()
            | TranscodeReason::AUDIO_BIT_DEPTH_NOT_SUPPORTED.bits()
            | TranscodeReason::AUDIO_IS_EXTERNAL.bits(),
    )
}

const fn video_codec_reasons() -> TranscodeReason {
    TranscodeReason::from_bits_retain(
        TranscodeReason::VIDEO_RESOLUTION_NOT_SUPPORTED.bits()
            | TranscodeReason::ANAMORPHIC_VIDEO_NOT_SUPPORTED.bits()
            | TranscodeReason::INTERLACED_VIDEO_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_BIT_DEPTH_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_BITRATE_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_FRAMERATE_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_LEVEL_NOT_SUPPORTED.bits()
            | TranscodeReason::REF_FRAMES_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_PROFILE_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_ROTATION_NOT_SUPPORTED.bits(),
    )
}

const fn video_reasons() -> TranscodeReason {
    TranscodeReason::from_bits_retain(
        TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED.bits() | video_codec_reasons().bits(),
    )
}

const fn direct_stream_reasons() -> TranscodeReason {
    TranscodeReason::from_bits_retain(
        TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED.bits()
            | audio_codec_reasons().bits()
            | TranscodeReason::CONTAINER_NOT_SUPPORTED.bits()
            | TranscodeReason::VIDEO_CODEC_TAG_NOT_SUPPORTED.bits(),
    )
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
        ProfileConditionValue::Height | ProfileConditionValue::Width => {
            TranscodeReason::VIDEO_RESOLUTION_NOT_SUPPORTED
        }
        ProfileConditionValue::IsAnamorphic => TranscodeReason::ANAMORPHIC_VIDEO_NOT_SUPPORTED,
        ProfileConditionValue::IsInterlaced => TranscodeReason::INTERLACED_VIDEO_NOT_SUPPORTED,
        ProfileConditionValue::IsSecondaryAudio => TranscodeReason::SECONDARY_AUDIO_NOT_SUPPORTED,
        ProfileConditionValue::NumStreams => TranscodeReason::STREAM_COUNT_EXCEEDS_LIMIT,
        ProfileConditionValue::RefFrames => TranscodeReason::REF_FRAMES_NOT_SUPPORTED,
        ProfileConditionValue::VideoBitDepth => TranscodeReason::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
        ProfileConditionValue::VideoBitrate => TranscodeReason::VIDEO_BITRATE_NOT_SUPPORTED,
        ProfileConditionValue::VideoCodecTag => TranscodeReason::VIDEO_CODEC_TAG_NOT_SUPPORTED,
        ProfileConditionValue::VideoFramerate => TranscodeReason::VIDEO_FRAMERATE_NOT_SUPPORTED,
        ProfileConditionValue::VideoLevel => TranscodeReason::VIDEO_LEVEL_NOT_SUPPORTED,
        ProfileConditionValue::VideoProfile => TranscodeReason::VIDEO_PROFILE_NOT_SUPPORTED,
        ProfileConditionValue::VideoRangeType => TranscodeReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED,
        ProfileConditionValue::VideoRotation => TranscodeReason::VIDEO_ROTATION_NOT_SUPPORTED,
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
