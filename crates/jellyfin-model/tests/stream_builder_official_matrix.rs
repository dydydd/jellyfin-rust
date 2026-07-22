use std::{fs, path::PathBuf};

use jellyfin_model::{
    MediaOptions, MediaSourceInfo, MediaStreamProtocol, PlayMethod, StreamBuilder, TranscodeReason,
};
use uuid::Uuid;

const OFFICIAL_TESTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../jellyfin/tests/Jellyfin.Model.Tests/Dlna/StreamBuilderTests.cs"
));

#[test]
fn official_build_video_item_simple_matrix() {
    let cases = build_video_item_simple_cases();
    assert_eq!(cases.len(), 176, "the upstream matrix changed");

    let mut failures = Vec::new();
    for case in cases {
        let options = media_options(&case.device, &case.source);
        let result = StreamBuilder::default()
            .get_optimal_video_stream(&options)
            .unwrap_or_else(|error| panic!("{} / {}: {error}", case.device, case.source))
            .unwrap_or_else(|| panic!("{} / {}: no stream", case.device, case.source));

        if case
            .method
            .is_some_and(|expected| result.play_method != expected)
            || result.transcode_reasons != case.reasons
        {
            failures.push(format!(
                "{} / {}: method {:?} != {:?}, reasons {:?} != {:?}",
                case.device,
                case.source,
                result.play_method,
                case.method,
                result.transcode_reasons,
                case.reasons
            ));
            continue;
        }

        if case.method == Some(PlayMethod::Transcode) {
            let protocol_matches = match case.protocol.as_str() {
                "HLS.mp4" => {
                    result.sub_protocol == MediaStreamProtocol::Hls
                        && result.container.as_deref() == Some("mp4")
                }
                "HLS.ts" => {
                    result.sub_protocol == MediaStreamProtocol::Hls
                        && result.container.as_deref() == Some("ts")
                }
                "http" => result.sub_protocol == MediaStreamProtocol::Http,
                unexpected => panic!("unexpected official protocol {unexpected}"),
            };
            if !protocol_matches {
                failures.push(format!(
                    "{} / {}: protocol {} produced {:?}.{:?}",
                    case.device, case.source, case.protocol, result.sub_protocol, result.container
                ));
            }
            let url = result.to_url(Some("media:"), Some("ACCESSTOKEN"), None);
            let path = url.split('?').next().unwrap_or_default();
            let expected_suffix = if case.protocol.starts_with("HLS.") {
                "/master.m3u8".to_owned()
            } else {
                format!(
                    "/stream.{}",
                    result.container.as_deref().unwrap_or_default()
                )
            };
            if !path.ends_with(&expected_suffix) {
                failures.push(format!(
                    "{} / {}: URL path {path} does not end with {expected_suffix}",
                    case.device, case.source
                ));
            }
            validate_transcode_mode(&case, &result, &mut failures);
        } else if case.method.is_none() && result.sub_protocol != MediaStreamProtocol::Http {
            failures.push(format!(
                "{} / {}: null play method should use HTTP fallback, got {:?}",
                case.device, case.source, result.sub_protocol
            ));
        } else if case.method == Some(PlayMethod::DirectPlay) {
            let source = result.media_source.as_ref().expect("media source");
            if !source.container.as_deref().is_some_and(|containers| {
                result.container.as_deref().is_some_and(|container| {
                    containers
                        .split(',')
                        .any(|candidate| candidate.eq_ignore_ascii_case(container))
                })
            }) {
                failures.push(format!(
                    "{} / {}: direct-play container {:?} not in source {:?}",
                    case.device, case.source, result.container, source.container
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} official StreamBuilder cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[derive(Debug)]
struct OfficialCase {
    device: String,
    source: String,
    method: Option<PlayMethod>,
    reasons: TranscodeReason,
    mode: String,
    protocol: String,
}

fn build_video_item_simple_cases() -> Vec<OfficialCase> {
    let section = OFFICIAL_TESTS
        .split("[Theory]")
        .nth(1)
        .expect("BuildVideoItemSimple theory")
        .split("public async Task BuildVideoItemSimple")
        .next()
        .expect("BuildVideoItemSimple body");

    section
        .lines()
        .filter_map(|line| line.trim().strip_prefix("[InlineData("))
        .map(|line| {
            let arguments = split_csharp_arguments(
                line.split_once(")] ")
                    .map_or_else(|| line.strip_suffix(")]").unwrap_or(line), |(args, _)| args),
            );
            let device = csharp_string(&arguments[0]);
            let source = csharp_string(&arguments[1]);
            let method = parse_play_method(&arguments[2]);
            let reasons = arguments
                .get(3)
                .map_or(TranscodeReason::NONE, |value| parse_reasons(value));
            let mode = arguments
                .get(4)
                .map_or_else(|| "DirectStream".to_owned(), |value| csharp_string(value));
            let protocol = arguments
                .get(5)
                .map_or_else(|| "HLS.ts".to_owned(), |value| csharp_string(value));
            OfficialCase {
                device,
                source,
                method,
                reasons,
                mode,
                protocol,
            }
        })
        .collect()
}

fn validate_transcode_mode(
    case: &OfficialCase,
    result: &jellyfin_model::StreamInfo,
    failures: &mut Vec<String>,
) {
    let source = result.media_source.as_ref().expect("media source");
    let source_video_codec = source
        .video_stream()
        .and_then(|stream| stream.codec.as_deref());
    let video_is_copied = source_video_codec.is_some_and(|codec| {
        result
            .video_codecs
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(codec))
    });
    let target_audio = result
        .audio_stream_index
        .and_then(|index| source.media_stream(jellyfin_model::MediaStreamType::Audio, index));
    let audio_is_copied = target_audio
        .and_then(|stream| stream.codec.as_deref())
        .is_some_and(|codec| {
            result
                .audio_codecs
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(codec))
        });

    let matches = match case.mode.as_str() {
        "Remux" => video_is_copied && audio_is_copied,
        "DirectStream" => {
            video_is_copied
                && (target_audio.is_some_and(|audio| audio.is_external)
                    || case
                        .reasons
                        .contains(TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED)
                    || !audio_is_copied)
        }
        "Transcode" => {
            let permits_video_copy = case
                .reasons
                .contains(TranscodeReason::CONTAINER_NOT_SUPPORTED)
                || case
                    .reasons
                    .contains(TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT)
                || case.reasons.contains(TranscodeReason::DIRECT_PLAY_ERROR)
                || case
                    .reasons
                    .contains(TranscodeReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED);
            permits_video_copy || !video_is_copied
        }
        unexpected => panic!("unexpected official transcode mode {unexpected}"),
    };
    if !matches {
        failures.push(format!(
            "{} / {}: mode {} produced video codecs {:?}, audio codecs {:?}, audio index {:?}",
            case.device,
            case.source,
            case.mode,
            result.video_codecs,
            result.audio_codecs,
            result.audio_stream_index
        ));
    }
}

fn split_csharp_arguments(input: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut parentheses = 0_u32;
    for character in input.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                current.push(character);
            }
            '(' if !quoted => {
                parentheses += 1;
                current.push(character);
            }
            ')' if !quoted => {
                parentheses = parentheses.saturating_sub(1);
                current.push(character);
            }
            ',' if !quoted && parentheses == 0 => {
                arguments.push(current.trim().to_owned());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    arguments.push(current.trim().to_owned());
    arguments
}

fn csharp_string(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or_else(|| panic!("not a C# string: {value}"))
        .to_owned()
}

fn parse_play_method(value: &str) -> Option<PlayMethod> {
    match value.trim().strip_prefix("PlayMethod.").unwrap_or(value) {
        "DirectPlay" => Some(PlayMethod::DirectPlay),
        "DirectStream" => Some(PlayMethod::DirectStream),
        "Transcode" => Some(PlayMethod::Transcode),
        "null" => None,
        unexpected => panic!("unexpected play method {unexpected}"),
    }
}

fn parse_reasons(value: &str) -> TranscodeReason {
    if value.trim() == "(TranscodeReason)0" {
        return TranscodeReason::NONE;
    }
    value
        .split('|')
        .fold(TranscodeReason::NONE, |reasons, part| {
            let name = part
                .trim()
                .strip_prefix("TranscodeReason.")
                .unwrap_or_else(|| panic!("unexpected reason expression {part}"));
            reasons
                | match name {
                    "ContainerNotSupported" => TranscodeReason::CONTAINER_NOT_SUPPORTED,
                    "VideoCodecNotSupported" => TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED,
                    "AudioCodecNotSupported" => TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED,
                    "AudioIsExternal" => TranscodeReason::AUDIO_IS_EXTERNAL,
                    "SecondaryAudioNotSupported" => TranscodeReason::SECONDARY_AUDIO_NOT_SUPPORTED,
                    "VideoProfileNotSupported" => TranscodeReason::VIDEO_PROFILE_NOT_SUPPORTED,
                    "VideoBitDepthNotSupported" => TranscodeReason::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
                    "VideoFramerateNotSupported" => TranscodeReason::VIDEO_FRAMERATE_NOT_SUPPORTED,
                    "AudioChannelsNotSupported" => TranscodeReason::AUDIO_CHANNELS_NOT_SUPPORTED,
                    "ContainerBitrateExceedsLimit" => {
                        TranscodeReason::CONTAINER_BITRATE_EXCEEDS_LIMIT
                    }
                    "VideoBitrateNotSupported" => TranscodeReason::VIDEO_BITRATE_NOT_SUPPORTED,
                    "DirectPlayError" => TranscodeReason::DIRECT_PLAY_ERROR,
                    "VideoRangeTypeNotSupported" => TranscodeReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED,
                    "VideoCodecTagNotSupported" => TranscodeReason::VIDEO_CODEC_TAG_NOT_SUPPORTED,
                    "StreamCountExceedsLimit" => TranscodeReason::STREAM_COUNT_EXCEEDS_LIMIT,
                    "VideoRotationNotSupported" => TranscodeReason::VIDEO_ROTATION_NOT_SUPPORTED,
                    unexpected => panic!("unmapped transcode reason {unexpected}"),
                }
        })
}

fn media_options(device: &str, source: &str) -> MediaOptions {
    let source: MediaSourceInfo = read_fixture("MediaSourceInfo", source);
    let media_source_id = source.id.clone();
    MediaOptions {
        item_id: Uuid::parse_str("11D229B7-2D48-4B95-9F9B-49F6AB75E613").unwrap(),
        media_source_id,
        media_sources: vec![source],
        device_id: Some("test-deviceId".into()),
        profile: read_fixture("DeviceProfile", device),
        allow_audio_stream_copy: true,
        allow_video_stream_copy: true,
        enable_direct_stream: false,
        ..MediaOptions::default()
    }
}

fn read_fixture<T: serde::de::DeserializeOwned>(kind: &str, name: &str) -> T {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../jellyfin/tests/Jellyfin.Model.Tests/Test Data")
        .join(format!("{kind}-{name}.json"));
    let json = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("could not parse {}: {error}", path.display()))
}
