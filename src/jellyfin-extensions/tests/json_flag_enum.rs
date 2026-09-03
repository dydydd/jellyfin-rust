use jellyfin_extensions::json::{FlagEnum, flags};
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
struct TranscodeReason(u64);

impl TranscodeReason {
    const CONTAINER_NOT_SUPPORTED: u64 = 1 << 0;
    const AUDIO_IS_EXTERNAL: u64 = 1 << 4;
    const VIDEO_BIT_DEPTH_NOT_SUPPORTED: u64 = 1 << 9;
}

impl FlagEnum for TranscodeReason {
    fn bits(self) -> u64 {
        self.0
    }

    fn ordered_flags() -> &'static [(u64, &'static str)] {
        &[
            (Self::CONTAINER_NOT_SUPPORTED, "ContainerNotSupported"),
            (Self::AUDIO_IS_EXTERNAL, "AudioIsExternal"),
            (
                Self::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
                "VideoBitDepthNotSupported",
            ),
        ]
    }
}

#[derive(Serialize)]
struct Value(#[serde(serialize_with = "flags::serialize")] TranscodeReason);

#[test]
fn serialize_two_transcode_reasons() {
    let value = TranscodeReason(
        TranscodeReason::AUDIO_IS_EXTERNAL | TranscodeReason::CONTAINER_NOT_SUPPORTED,
    );
    assert_eq!(
        serde_json::to_string(&Value(value)).unwrap(),
        r#"["ContainerNotSupported","AudioIsExternal"]"#
    );
}

#[test]
fn serialize_three_transcode_reasons() {
    let value = TranscodeReason(
        TranscodeReason::AUDIO_IS_EXTERNAL
            | TranscodeReason::CONTAINER_NOT_SUPPORTED
            | TranscodeReason::VIDEO_BIT_DEPTH_NOT_SUPPORTED,
    );
    assert_eq!(
        serde_json::to_string(&Value(value)).unwrap(),
        r#"["ContainerNotSupported","AudioIsExternal","VideoBitDepthNotSupported"]"#
    );
}

#[test]
fn serialize_zero_transcode_reasons_as_empty_array() {
    assert_eq!(
        serde_json::to_string(&Value(TranscodeReason(0))).unwrap(),
        "[]"
    );
}
