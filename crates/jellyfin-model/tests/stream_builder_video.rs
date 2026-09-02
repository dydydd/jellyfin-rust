use jellyfin_model::{
    CodecProfile, CodecType, ContainerProfile, DeviceProfile, DirectPlayProfile, DlnaProfileType,
    MediaOptions, MediaSourceInfo, MediaStream, MediaStreamProtocol, MediaStreamType, PlayMethod,
    ProfileCondition, ProfileConditionType, ProfileConditionValue, StreamBuilder,
    StreamBuilderError, SubtitleDeliveryMethod, SubtitleProfile, TranscodeReason,
    TranscodingProfile, VideoRangeType, VideoType,
};
use uuid::Uuid;

#[test]
fn video_direct_play_direct_stream_and_transcode_matrix() {
    struct Case {
        name: &'static str,
        configure: fn(&mut MediaOptions),
        method: PlayMethod,
        reasons: TranscodeReason,
        container: &'static str,
    }

    let cases = [
        Case {
            name: "matching profile direct plays",
            configure: |_| {},
            method: PlayMethod::DirectPlay,
            reasons: TranscodeReason::NONE,
            container: "mp4",
        },
        Case {
            name: "container-only failure direct streams when enabled",
            configure: |options| {
                options.media_sources[0].container = Some("mkv".into());
                options.enable_direct_stream = true;
            },
            method: PlayMethod::DirectStream,
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: "mkv",
        },
        Case {
            name: "container failure transcodes when direct stream is disabled",
            configure: |options| options.media_sources[0].container = Some("mkv".into()),
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: "mp4",
        },
        Case {
            name: "unsupported video codec transcodes",
            configure: |options| {
                options.media_sources[0].media_streams[0].codec = Some("vp9".into());
            },
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED,
            container: "mp4",
        },
        Case {
            name: "unsupported audio codec transcodes audio with video copy",
            configure: |options| {
                options.media_sources[0].media_streams[1].codec = Some("ac3".into());
            },
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED,
            container: "mp4",
        },
        Case {
            name: "external audio reports its dedicated reason",
            configure: |options| options.media_sources[0].media_streams[1].is_external = true,
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::AUDIO_IS_EXTERNAL,
            container: "mp4",
        },
        Case {
            name: "bitrate limit forces transcode",
            configure: |options| options.max_bitrate = Some(1_000_000),
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT,
            container: "mp4",
        },
        Case {
            name: "disc source disables ordinary direct play",
            configure: |options| {
                options.media_sources[0].video_type = Some(VideoType::BluRay);
            },
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::NONE,
            container: "mp4",
        },
        Case {
            name: "missing direct profiles returns direct play error",
            configure: |options| options.profile.direct_play_profiles.clear(),
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::DIRECT_PLAY_ERROR,
            container: "mp4",
        },
        Case {
            name: "missing video stream reports unsupported codec",
            configure: |options| {
                options.media_sources[0]
                    .media_streams
                    .retain(|stream| stream.stream_type != MediaStreamType::Video);
            },
            method: PlayMethod::Transcode,
            reasons: TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED,
            container: "mp4",
        },
    ];

    for case in cases {
        let mut options = base_options();
        (case.configure)(&mut options);
        let result = build(&options);
        assert_eq!(result.play_method, case.method, "{}", case.name);
        assert_eq!(result.transcode_reasons, case.reasons, "{}", case.name);
        assert_eq!(
            result.container.as_deref(),
            Some(case.container),
            "{}",
            case.name
        );
    }
}

#[test]
fn video_codec_condition_failure_reason_matrix() {
    struct Case {
        property: ProfileConditionValue,
        condition_type: ProfileConditionType,
        value: &'static str,
        expected: TranscodeReason,
    }
    let cases = [
        Case {
            property: ProfileConditionValue::Width,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "640",
            expected: TranscodeReason::VIDEO_RESOLUTION_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::Height,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "360",
            expected: TranscodeReason::VIDEO_RESOLUTION_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoBitDepth,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "7",
            expected: TranscodeReason::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoBitrate,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "1000000",
            expected: TranscodeReason::VIDEO_BITRATE_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoFramerate,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "20",
            expected: TranscodeReason::VIDEO_FRAMERATE_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoLevel,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "40",
            expected: TranscodeReason::VIDEO_LEVEL_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoProfile,
            condition_type: ProfileConditionType::EqualsAny,
            value: "main|baseline",
            expected: TranscodeReason::VIDEO_PROFILE_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoRangeType,
            condition_type: ProfileConditionType::EqualsAny,
            value: "HDR10|HLG",
            expected: TranscodeReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoCodecTag,
            condition_type: ProfileConditionType::Equals,
            value: "hvc1",
            expected: TranscodeReason::VIDEO_CODEC_TAG_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::RefFrames,
            condition_type: ProfileConditionType::LessThanEqual,
            value: "0",
            expected: TranscodeReason::REF_FRAMES_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::VideoRotation,
            condition_type: ProfileConditionType::Equals,
            value: "90",
            expected: TranscodeReason::VIDEO_ROTATION_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::IsAnamorphic,
            condition_type: ProfileConditionType::NotEquals,
            value: "true",
            expected: TranscodeReason::ANAMORPHIC_VIDEO_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::IsInterlaced,
            condition_type: ProfileConditionType::Equals,
            value: "true",
            expected: TranscodeReason::INTERLACED_VIDEO_NOT_SUPPORTED,
        },
    ];

    for case in cases {
        let mut options = base_options();
        if case.property == ProfileConditionValue::IsAnamorphic {
            options.media_sources[0].media_streams[0].is_anamorphic = Some(true);
        }
        options.profile.codec_profiles.push(CodecProfile {
            profile_type: CodecType::Video,
            codec: Some("h264".into()),
            conditions: vec![condition(
                case.property,
                case.condition_type,
                case.value,
                true,
            )],
            ..CodecProfile::default()
        });
        let result = build(&options);
        assert_eq!(
            result.play_method,
            PlayMethod::Transcode,
            "{:?}",
            case.property
        );
        assert_eq!(
            result.transcode_reasons, case.expected,
            "{:?}",
            case.property
        );
    }
}

#[test]
fn container_and_audio_profile_conditions_report_official_reasons() {
    let mut options = base_options();
    options.profile.container_profiles.push(ContainerProfile {
        profile_type: DlnaProfileType::Video,
        container: Some("mp4".into()),
        conditions: vec![condition(
            ProfileConditionValue::NumStreams,
            ProfileConditionType::LessThanEqual,
            "2",
            true,
        )],
        ..ContainerProfile::default()
    });
    let result = build(&options);
    assert_eq!(
        result.transcode_reasons,
        TranscodeReason::STREAM_COUNT_EXCEEDS_LIMIT
    );

    let mut options = base_options();
    let mut second = options.media_sources[0].media_streams[1].clone();
    second.index = 3;
    second.is_default = false;
    options.media_sources[0].media_streams.push(second);
    options.audio_stream_index = Some(3);
    options.media_source_id = Some("video-1".into());
    options.profile.codec_profiles.push(CodecProfile {
        profile_type: CodecType::VideoAudio,
        codec: Some("aac".into()),
        conditions: vec![condition(
            ProfileConditionValue::IsSecondaryAudio,
            ProfileConditionType::Equals,
            "false",
            false,
        )],
        ..CodecProfile::default()
    });
    let result = build(&options);
    assert_eq!(result.audio_stream_index, Some(3));
    assert_eq!(
        result.transcode_reasons,
        TranscodeReason::SECONDARY_AUDIO_NOT_SUPPORTED
    );
}

#[test]
fn transcoding_targets_apply_hls_filters_and_video_limits() {
    let mut options = base_options();
    options.media_sources[0].media_streams[0].codec = Some("vp9".into());
    options.max_bitrate = Some(4_000_000);
    let profile = &mut options.profile.transcoding_profiles[0];
    profile.container = "ts".into();
    profile.protocol = MediaStreamProtocol::Hls;
    profile.video_codec = "mpeg2video,h264".into();
    profile.audio_codec = "flac,aac".into();
    profile.conditions = vec![
        condition(
            ProfileConditionValue::Width,
            ProfileConditionType::LessThanEqual,
            "960",
            true,
        ),
        condition(
            ProfileConditionValue::Height,
            ProfileConditionType::LessThanEqual,
            "540",
            true,
        ),
        condition(
            ProfileConditionValue::VideoBitrate,
            ProfileConditionType::LessThanEqual,
            "2500000",
            true,
        ),
        condition(
            ProfileConditionValue::VideoFramerate,
            ProfileConditionType::LessThanEqual,
            "20",
            true,
        ),
    ];

    let result = build(&options);
    assert_eq!(result.video_codecs, ["h264"]);
    assert_eq!(result.audio_codecs, ["aac"]);
    assert_eq!(result.max_width, Some(960));
    assert_eq!(result.max_height, Some(540));
    assert_eq!(result.max_framerate, Some(20.0));
    assert!(result.video_bitrate.unwrap() <= 2_500_000);
    assert_eq!(result.sub_protocol, MediaStreamProtocol::Hls);
}

#[test]
fn subtitle_delivery_matrix_covers_embed_external_hls_and_encode() {
    struct Case {
        name: &'static str,
        configure: fn(&mut MediaOptions),
        expected: SubtitleDeliveryMethod,
    }
    let cases = [
        Case {
            name: "external direct-play subtitle",
            configure: |_| {},
            expected: SubtitleDeliveryMethod::External,
        },
        Case {
            name: "embedded subtitle direct plays",
            configure: |options| {
                options.media_sources[0].media_streams[2].is_external = false;
                options.profile.subtitle_profiles.insert(
                    0,
                    SubtitleProfile {
                        format: "srt".into(),
                        method: SubtitleDeliveryMethod::Embed,
                        container: Some("mp4".into()),
                        ..SubtitleProfile::default()
                    },
                );
            },
            expected: SubtitleDeliveryMethod::Embed,
        },
        Case {
            name: "internal subtitle uses HLS delivery while transcoding",
            configure: |options| {
                options.profile.direct_play_profiles.clear();
                options.media_sources[0].media_streams[2].is_external = false;
                options.media_sources[0].media_streams[2].supports_external_stream = true;
                options.profile.subtitle_profiles = vec![SubtitleProfile {
                    format: "srt".into(),
                    method: SubtitleDeliveryMethod::Hls,
                    ..SubtitleProfile::default()
                }];
            },
            expected: SubtitleDeliveryMethod::Hls,
        },
        Case {
            name: "unsupported subtitle burns in",
            configure: |options| options.profile.subtitle_profiles.clear(),
            expected: SubtitleDeliveryMethod::Encode,
        },
    ];

    for case in cases {
        let mut options = base_options();
        (case.configure)(&mut options);
        let result = build(&options);
        assert_eq!(
            result.subtitle_delivery_method, case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn explicit_streams_require_media_source_and_forced_modes_bypass_profiles() {
    let mut options = base_options();
    options.audio_stream_index = Some(1);
    assert_eq!(
        StreamBuilder::default().get_optimal_video_stream(&options),
        Err(StreamBuilderError::MissingMediaSourceId)
    );

    options.media_source_id = Some("video-1".into());
    options.force_direct_stream = true;
    options.profile.direct_play_profiles.clear();
    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::DirectStream);
    assert_eq!(result.audio_stream_index, Some(1));
}

#[test]
fn user_policy_flags_gate_remote_direct_play_and_transcoding() {
    let mut options = base_options();
    options.media_sources[0].is_remote = true;
    options.force_remote_source_transcoding = true;
    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::Transcode);

    let mut options = base_options();
    options.enable_transcoding = false;
    options.media_sources[0].media_streams[0].codec = Some("vp9".into());
    let result = StreamBuilder::default()
        .get_optimal_video_stream(&options)
        .unwrap();
    assert!(result.is_none());

    let mut options = base_options();
    options.media_sources[0].container = Some("mkv".into());
    options.enable_direct_stream = true;
    options.enable_playback_remuxing = false;
    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::Transcode);
}

fn base_options() -> MediaOptions {
    MediaOptions {
        item_id: Uuid::new_v4(),
        device_id: Some("test-device".into()),
        enable_direct_stream: false,
        allow_video_stream_copy: true,
        allow_audio_stream_copy: true,
        media_sources: vec![MediaSourceInfo {
            id: Some("video-1".into()),
            container: Some("mp4".into()),
            bitrate: Some(2_600_000),
            default_audio_stream_index: Some(1),
            default_subtitle_stream_index: Some(2),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".into()),
                    codec_tag: Some("avc1".into()),
                    stream_type: MediaStreamType::Video,
                    width: Some(1280),
                    height: Some(720),
                    bit_rate: Some(2_000_000),
                    bit_depth: Some(8),
                    average_frame_rate: Some(23.976),
                    real_frame_rate: Some(23.976),
                    profile: Some("High".into()),
                    level: Some(41.0),
                    ref_frames: Some(1),
                    rotation: Some(0),
                    is_anamorphic: Some(false),
                    video_range_type: VideoRangeType::Sdr,
                    is_default: true,
                    ..MediaStream::default()
                },
                MediaStream {
                    codec: Some("aac".into()),
                    stream_type: MediaStreamType::Audio,
                    index: 1,
                    channels: Some(2),
                    bit_rate: Some(164_000),
                    sample_rate: Some(48_000),
                    profile: Some("LC".into()),
                    is_default: true,
                    ..MediaStream::default()
                },
                MediaStream {
                    codec: Some("srt".into()),
                    stream_type: MediaStreamType::Subtitle,
                    index: 2,
                    is_external: true,
                    supports_external_stream: true,
                    ..MediaStream::default()
                },
            ],
            ..MediaSourceInfo::default()
        }],
        profile: DeviceProfile {
            max_streaming_bitrate: Some(120_000_000),
            direct_play_profiles: vec![DirectPlayProfile {
                container: "mp4".into(),
                video_codec: Some("h264,hevc".into()),
                audio_codec: Some("aac".into()),
                profile_type: DlnaProfileType::Video,
            }],
            transcoding_profiles: vec![TranscodingProfile {
                container: "mp4".into(),
                profile_type: DlnaProfileType::Video,
                video_codec: "h264".into(),
                audio_codec: "aac".into(),
                protocol: MediaStreamProtocol::Http,
                max_audio_channels: Some("2".into()),
                ..TranscodingProfile::default()
            }],
            subtitle_profiles: vec![SubtitleProfile {
                format: "srt".into(),
                method: SubtitleDeliveryMethod::External,
                ..SubtitleProfile::default()
            }],
            ..DeviceProfile::default()
        },
        ..MediaOptions::default()
    }
}

fn condition(
    property: ProfileConditionValue,
    condition_type: ProfileConditionType,
    value: &str,
    required: bool,
) -> ProfileCondition {
    ProfileCondition {
        property,
        condition: condition_type,
        value: value.into(),
        is_required: required,
    }
}

fn build(options: &MediaOptions) -> jellyfin_model::StreamInfo {
    StreamBuilder::default()
        .get_optimal_video_stream(options)
        .unwrap()
        .unwrap()
}
