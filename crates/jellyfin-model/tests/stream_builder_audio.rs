use jellyfin_model::{
    CodecProfile, CodecType, DeviceProfile, DirectPlayProfile, DlnaProfileType, EncodingContext,
    MediaOptions, MediaProtocol, MediaSourceInfo, MediaStream, MediaStreamProtocol,
    MediaStreamType, PlayMethod, ProfileCondition, ProfileConditionType, ProfileConditionValue,
    StreamBuilder, StreamBuilderError, TranscodeReason, TranscodeSeekInfo, TranscodingProfile,
};
use uuid::Uuid;

#[test]
fn audio_direct_play_and_transcoding_decision_matrix() {
    struct Case {
        name: &'static str,
        configure: fn(&mut MediaOptions),
        method: Option<PlayMethod>,
        reasons: TranscodeReason,
        container: Option<&'static str>,
        protocol: MediaStreamProtocol,
    }

    let cases = [
        Case {
            name: "matching container and codec direct plays",
            configure: |_| {},
            method: Some(PlayMethod::DirectPlay),
            reasons: TranscodeReason::NONE,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "forced direct play bypasses profile matching",
            configure: |options| {
                options.force_direct_play = true;
                options.profile.direct_play_profiles.clear();
            },
            method: Some(PlayMethod::DirectPlay),
            reasons: TranscodeReason::NONE,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "forced direct stream bypasses profile matching",
            configure: |options| {
                options.force_direct_stream = true;
                options.profile.direct_play_profiles.clear();
            },
            method: Some(PlayMethod::DirectStream),
            reasons: TranscodeReason::NONE,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "codec-only match remuxes unsupported container",
            configure: |options| {
                options.media_sources[0].container = Some("wav".into());
                options.media_sources[0].transcoding_container = Some("mp4".into());
                options.enable_direct_stream = false;
            },
            method: Some(PlayMethod::DirectStream),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: Some("mp4"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "two unknown codecs compare equal for remux",
            configure: |options| {
                options.media_sources[0].container = Some("wav".into());
                options.media_sources[0].media_streams[0].codec = None;
                options.profile.direct_play_profiles[0].audio_codec = None;
            },
            method: Some(PlayMethod::DirectStream),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: Some("ts"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "HLS TS accepts the official MP3 remux codec",
            configure: |options| {
                options.media_sources[0].container = Some("wav".into());
                options.media_sources[0].transcoding_sub_protocol = MediaStreamProtocol::Hls;
            },
            method: Some(PlayMethod::DirectStream),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: Some("ts"),
            protocol: MediaStreamProtocol::Hls,
        },
        Case {
            name: "HLS TS rejects FLAC remux and transcodes",
            configure: |options| configure_flac_remux(options, "ts"),
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED
                | TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "HLS fragmented MP4 accepts FLAC remux",
            configure: |options| configure_flac_remux(options, "mp4"),
            method: Some(PlayMethod::DirectStream),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: Some("mp4"),
            protocol: MediaStreamProtocol::Hls,
        },
        Case {
            name: "HLS container override is case insensitive",
            configure: |options| {
                options.media_sources[0].container = Some("wav".into());
                options.media_sources[0].transcoding_sub_protocol = MediaStreamProtocol::Hls;
                options.profile.direct_play_profiles[0].container = "MP4".into();
            },
            method: Some(PlayMethod::DirectStream),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: Some("MP4"),
            protocol: MediaStreamProtocol::Hls,
        },
        Case {
            name: "supported container with wrong codec reports audio only",
            configure: |options| {
                options.media_sources[0].media_streams[0].codec = Some("flac".into());
            },
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "no matching container reports official three flags",
            configure: |options| {
                options.media_sources[0].container = Some("ogg".into());
                options.media_sources[0].media_streams[0].codec = Some("vorbis".into());
            },
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED
                | TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED
                | TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "known bitrate above maximum transcodes",
            configure: |options| options.max_bitrate = Some(64_000),
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "unknown local bitrate uses official 40 Mbps estimate",
            configure: |options| options.media_sources[0].bitrate = None,
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "remote media bypasses bitrate restrictions",
            configure: |options| {
                options.max_bitrate = Some(1);
                options.media_sources[0].is_remote = true;
            },
            method: Some(PlayMethod::DirectPlay),
            reasons: TranscodeReason::NONE,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "source direct play capability can force transcode",
            configure: |options| options.media_sources[0].supports_direct_play = false,
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::NONE,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "disabled direct play can force transcode",
            configure: |options| options.enable_direct_play = false,
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::NONE,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
        Case {
            name: "source direct stream capability blocks remux",
            configure: |options| {
                options.media_sources[0].container = Some("wav".into());
                options.media_sources[0].supports_direct_stream = false;
            },
            method: Some(PlayMethod::Transcode),
            reasons: TranscodeReason::CONTAINER_NOT_SUPPORTED,
            container: Some("mp3"),
            protocol: MediaStreamProtocol::Http,
        },
    ];

    for case in cases {
        let mut options = base_options();
        (case.configure)(&mut options);
        let result = StreamBuilder::default()
            .get_optimal_audio_stream(&options)
            .unwrap()
            .unwrap_or_else(|| panic!("{} returned no stream", case.name));

        assert_eq!(result.play_method, case.method.unwrap(), "{}", case.name);
        assert_eq!(result.transcode_reasons, case.reasons, "{}", case.name);
        assert_eq!(result.container.as_deref(), case.container, "{}", case.name);
        assert_eq!(result.sub_protocol, case.protocol, "{}", case.name);
    }
}

#[test]
fn audio_codec_condition_failure_matrix() {
    struct Case {
        property: ProfileConditionValue,
        value: &'static str,
        expected: TranscodeReason,
    }

    let cases = [
        Case {
            property: ProfileConditionValue::AudioChannels,
            value: "1",
            expected: TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::AudioBitrate,
            value: "64000",
            expected: TranscodeReason::AUDIO_BITRATE_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::AudioSampleRate,
            value: "32000",
            expected: TranscodeReason::AUDIO_SAMPLE_RATE_NOT_SUPPORTED,
        },
        Case {
            property: ProfileConditionValue::AudioBitDepth,
            value: "16",
            expected: TranscodeReason::AUDIO_BIT_DEPTH_NOT_SUPPORTED,
        },
    ];

    for case in cases {
        let mut options = base_options();
        options.profile.codec_profiles.push(CodecProfile {
            profile_type: CodecType::Audio,
            codec: Some("mp3".into()),
            conditions: vec![condition(
                case.property,
                ProfileConditionType::LessThanEqual,
                case.value,
                true,
            )],
            ..CodecProfile::default()
        });

        let result = StreamBuilder::default()
            .get_optimal_audio_stream(&options)
            .unwrap()
            .unwrap();
        assert_eq!(result.play_method, PlayMethod::Transcode);
        assert_eq!(result.transcode_reasons, case.expected);
    }
}

#[test]
fn optional_unknown_condition_and_apply_condition_follow_official_rules() {
    let mut options = base_options();
    options.media_sources[0].media_streams[0].channels = None;
    options.profile.codec_profiles.push(CodecProfile {
        profile_type: CodecType::Audio,
        codec: Some("mp3".into()),
        conditions: vec![condition(
            ProfileConditionValue::AudioChannels,
            ProfileConditionType::LessThanEqual,
            "2",
            false,
        )],
        ..CodecProfile::default()
    });
    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::DirectPlay);

    options.profile.codec_profiles[0].conditions[0].is_required = true;
    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::Transcode);
    assert_eq!(
        result.transcode_reasons,
        TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED
    );

    options.profile.codec_profiles[0].apply_conditions = vec![condition(
        ProfileConditionValue::AudioSampleRate,
        ProfileConditionType::Equals,
        "96000",
        true,
    )];
    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::DirectPlay);
}

#[test]
fn transcoding_profile_applies_audio_limits_and_bitrate_precedence() {
    let mut options = base_options();
    options.enable_direct_play = false;
    options.audio_transcoding_bitrate = Some(256_000);
    options.max_bitrate = Some(192_000);
    options.max_audio_channels = Some(1);
    options.profile.codec_profiles.push(CodecProfile {
        profile_type: CodecType::Audio,
        codec: Some("aac".into()),
        container: Some("m4a".into()),
        conditions: vec![
            condition(
                ProfileConditionValue::AudioBitrate,
                ProfileConditionType::LessThanEqual,
                "160000",
                true,
            ),
            condition(
                ProfileConditionValue::AudioSampleRate,
                ProfileConditionType::Equals,
                "48000",
                true,
            ),
            condition(
                ProfileConditionValue::AudioChannels,
                ProfileConditionType::LessThanEqual,
                "2",
                true,
            ),
        ],
        ..CodecProfile::default()
    });
    let profile = &mut options.profile.transcoding_profiles[0];
    profile.container = "m4a".into();
    profile.audio_codec = "aac".into();
    profile.protocol = MediaStreamProtocol::Hls;
    profile.max_audio_channels = Some("6".into());
    profile.estimate_content_length = true;
    profile.enable_mpegts_m2ts_mode = true;
    profile.enable_subtitles_in_manifest = true;
    profile.copy_timestamps = true;
    profile.transcode_seek_info = TranscodeSeekInfo::Bytes;
    profile.min_segments = 3;
    profile.segment_length = 6;
    profile.enable_audio_vbr_encoding = false;

    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::Transcode);
    assert_eq!(result.container.as_deref(), Some("m4a"));
    assert_eq!(result.sub_protocol, MediaStreamProtocol::Hls);
    assert_eq!(result.audio_codecs, ["aac"]);
    assert_eq!(result.audio_bitrate, Some(160_000));
    assert_eq!(result.audio_sample_rate, Some(48_000));
    assert_eq!(result.transcoding_max_audio_channels, Some(6));
    assert_eq!(result.global_max_audio_channels, Some(1));
    assert_eq!(result.target_audio_channels(Some("aac")), Some(1));
    assert_eq!(result.transcode_seek_info, TranscodeSeekInfo::Bytes);
    assert!(result.estimate_content_length);
    assert!(result.enable_mpegts_m2ts_mode);
    assert!(result.enable_subtitles_in_manifest);
    assert!(result.copy_timestamps);
    assert_eq!(result.min_segments, Some(3));
    assert_eq!(result.segment_length, Some(6));
    assert!(!result.enable_audio_vbr_encoding);
}

#[test]
fn transcoder_capability_context_and_source_support_gate_profiles() {
    let mut options = base_options();
    options.enable_direct_play = false;

    let no_codec = StreamBuilder::with_encodable_audio_codecs(["flac"])
        .get_optimal_audio_stream(&options)
        .unwrap()
        .unwrap();
    assert_eq!(no_codec.play_method, PlayMethod::Transcode);
    assert_eq!(no_codec.container, None);

    options.profile.transcoding_profiles[0].audio_codec.clear();
    let empty_codec = build(&options);
    assert_eq!(empty_codec.container, None);
    options.profile.transcoding_profiles[0].audio_codec = "mp3".into();

    options.context = EncodingContext::Static;
    let wrong_context = build(&options);
    assert_eq!(wrong_context.container, None);

    options.profile.transcoding_profiles[0].context = EncodingContext::Static;
    options.media_sources[0].supports_transcoding = false;
    assert_eq!(
        StreamBuilder::default()
            .get_optimal_audio_stream(&options)
            .unwrap(),
        None
    );
}

#[test]
fn source_filter_sorting_and_container_normalization_match_official_order() {
    let mut options = base_options();
    options.media_source_id = Some("MUSIC-2".into());
    let mut second = options.media_sources[0].clone();
    second.id = Some("music-2".into());
    second.container = Some("aac,mp3".into());
    second.media_streams[0].codec = Some("mp3".into());
    options.media_sources.push(second);

    let filtered = build(&options);
    assert_eq!(filtered.media_source_id(), Some("music-2"));
    assert_eq!(filtered.container.as_deref(), Some("mp3"));

    options.media_source_id = None;
    options.media_sources[0].protocol = MediaProtocol::Http;
    options.media_sources[1].protocol = MediaProtocol::File;
    options.media_sources[1].container = Some("wav".into());
    let sorted = build(&options);
    assert_eq!(sorted.media_source_id(), Some("music-2"));
    assert_eq!(sorted.play_method, PlayMethod::DirectStream);
}

#[test]
fn webm_profile_does_not_direct_play_matroska_audio() {
    let mut options = base_options();
    options.media_sources[0].container = Some("mkv,webm".into());
    options.media_sources[0].media_streams[0].codec = Some("opus".into());
    options.profile.direct_play_profiles = vec![audio_profile("webm", "opus")];

    let result = build(&options);
    assert_eq!(result.play_method, PlayMethod::DirectStream);
    assert_eq!(
        result.transcode_reasons,
        TranscodeReason::CONTAINER_NOT_SUPPORTED
    );
}

#[test]
fn validation_and_missing_audio_errors_are_explicit() {
    let mut options = base_options();
    options.item_id = Uuid::nil();
    options.device_id = None;
    assert_eq!(
        StreamBuilder::default().get_optimal_audio_stream(&options),
        Err(StreamBuilderError::MissingDeviceId)
    );

    options.item_id = Uuid::new_v4();
    options.media_sources[0].media_streams.clear();
    assert_eq!(
        StreamBuilder::default().get_optimal_audio_stream(&options),
        Err(StreamBuilderError::MissingAudioStream)
    );
}

fn base_options() -> MediaOptions {
    MediaOptions {
        item_id: Uuid::new_v4(),
        device_id: Some("test-device".into()),
        media_sources: vec![MediaSourceInfo {
            id: Some("music-1".into()),
            protocol: MediaProtocol::File,
            container: Some("mp3".into()),
            bitrate: Some(128_000),
            media_streams: vec![MediaStream {
                codec: Some("mp3".into()),
                stream_type: MediaStreamType::Audio,
                channels: Some(2),
                bit_rate: Some(128_000),
                sample_rate: Some(44_100),
                bit_depth: Some(24),
                is_default: true,
                ..MediaStream::default()
            }],
            ..MediaSourceInfo::default()
        }],
        profile: DeviceProfile {
            direct_play_profiles: vec![audio_profile("mp3", "mp3")],
            transcoding_profiles: vec![TranscodingProfile {
                container: "mp3".into(),
                profile_type: DlnaProfileType::Audio,
                audio_codec: "mp3".into(),
                ..TranscodingProfile::default()
            }],
            ..DeviceProfile::default()
        },
        ..MediaOptions::default()
    }
}

fn audio_profile(container: &str, codec: &str) -> DirectPlayProfile {
    DirectPlayProfile {
        container: container.into(),
        audio_codec: Some(codec.into()),
        profile_type: DlnaProfileType::Audio,
        ..DirectPlayProfile::default()
    }
}

fn configure_flac_remux(options: &mut MediaOptions, container: &str) {
    options.media_sources[0].container = Some("wav".into());
    options.media_sources[0].media_streams[0].codec = Some("flac".into());
    options.media_sources[0].transcoding_sub_protocol = MediaStreamProtocol::Hls;
    options.media_sources[0].transcoding_container = Some(container.into());
    options.profile.direct_play_profiles = vec![audio_profile("mp3", "flac")];
}

fn condition(
    property: ProfileConditionValue,
    condition_type: ProfileConditionType,
    value: &str,
    is_required: bool,
) -> ProfileCondition {
    ProfileCondition {
        condition: condition_type,
        property,
        value: value.into(),
        is_required,
    }
}

fn build(options: &MediaOptions) -> jellyfin_model::StreamInfo {
    StreamBuilder::default()
        .get_optimal_audio_stream(options)
        .unwrap()
        .unwrap()
}
