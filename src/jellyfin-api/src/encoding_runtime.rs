use jellyfin_controller::FfmpegCommand;

const DEFAULT_TRANSCODE_CHANNEL_LIMIT: i32 = 8;

/// Apply Jellyfin's request/configuration thread-count precedence.
///
/// `CpuCoreLimit` is request-scoped and wins when present. Otherwise the
/// persisted `EncodingThreadCount` is used. Non-positive values deliberately
/// emit `-threads 0`, which asks `FFmpeg` to select the worker count.
pub(crate) fn apply_thread_count(
    command: &mut FfmpegCommand,
    requested: Option<i32>,
    configured: i32,
) {
    let requested = requested.unwrap_or(configured);
    let available = std::thread::available_parallelism()
        .map_or(1, |count| i32::try_from(count.get()).unwrap_or(i32::MAX));
    let threads = if requested <= 0 {
        0
    } else {
        requested.min(available)
    };
    let output_index = command.arguments.len().saturating_sub(1);
    command.arguments.splice(
        output_index..output_index,
        ["-threads".to_owned(), threads.to_string()],
    );
}

/// Replace constant-bitrate audio arguments with Jellyfin's codec-specific
/// VBR mode when both the server configuration and client profile enable it.
pub(crate) fn apply_audio_vbr(
    command: &mut FfmpegCommand,
    codec: &str,
    bitrate: Option<i64>,
    channels: Option<i32>,
    enabled: bool,
) {
    let Some(bitrate) = bitrate.filter(|bitrate| *bitrate > 0) else {
        return;
    };
    if !enabled {
        return;
    }
    let channels = i64::from(channels.unwrap_or(2).max(1));
    let bitrate_per_channel = bitrate / channels;
    let codec = codec.to_ascii_lowercase();
    let replacement = match codec.as_str() {
        "libfdk_aac" => Some(vec![
            "-vbr:a".to_owned(),
            match bitrate_per_channel {
                ..32_000 => "1",
                32_000..48_000 => "2",
                48_000..64_000 => "3",
                64_000..96_000 => "4",
                _ => "5",
            }
            .to_owned(),
        ]),
        "mp3" | "libmp3lame" if bitrate_per_channel > 48_000 && bitrate_per_channel < 122_500 => {
            Some(vec![
                "-qscale:a".to_owned(),
                match bitrate_per_channel {
                    ..64_000 => "6",
                    64_000..88_000 => "4",
                    88_000..112_000 => "2",
                    _ => "0",
                }
                .to_owned(),
            ])
        }
        "vorbis" | "libvorbis" => Some(vec![
            "-qscale:a".to_owned(),
            match bitrate_per_channel {
                ..40_000 => "0",
                40_000..56_000 => "2",
                56_000..80_000 => "4",
                80_000..112_000 => "6",
                _ => "8",
            }
            .to_owned(),
        ]),
        "aac_at" => {
            insert_before_audio_bitrate(command, ["-aac_at_mode:a".to_owned(), "2".to_owned()]);
            None
        }
        "mp3" | "libmp3lame" => {
            insert_before_audio_bitrate(command, ["-abr:a".to_owned(), "1".to_owned()]);
            None
        }
        _ => None,
    };
    if let Some(replacement) = replacement {
        replace_audio_bitrate(command, bitrate, replacement);
    }
}

pub(crate) fn audio_vbr_enabled(configured: bool, requested: Option<bool>) -> bool {
    configured && requested.unwrap_or(true)
}

/// Resolve the output channel count using Jellyfin's request precedence and
/// encoder/source caps.
///
/// Codec-qualified `audiochannels` is evaluated before the declared request
/// properties. The selected source channel count, the encoder's supported
/// maximum, and `TranscodingMaxAudioChannels` then cap that request.
pub(crate) fn output_audio_channels(
    codec: &str,
    source_channels: Option<i32>,
    stream_option_channels: Option<i32>,
    max_audio_channels: Option<i32>,
    audio_channels: Option<i32>,
    transcoding_max_audio_channels: Option<i32>,
) -> Option<i32> {
    let requested = stream_option_channels
        .or(max_audio_channels)
        .or(audio_channels)
        .or(transcoding_max_audio_channels);
    let mut output = match source_channels.filter(|channels| *channels > 0) {
        Some(source) => Some(requested.map_or(source, |requested| requested.min(source))),
        None => requested,
    };
    let encoder_limit = match codec.to_ascii_lowercase().as_str() {
        "mp3" | "libmp3lame" => 2,
        "libfdk_aac" | "ac3" | "eac3" | "dts" | "dca" | "mlp" | "truehd" => 6,
        _ => DEFAULT_TRANSCODE_CHANNEL_LIMIT,
    };
    output = Some(output.map_or(encoder_limit, |channels| channels.min(encoder_limit)));
    if let Some(maximum) = transcoding_max_audio_channels
        && output.is_some_and(|channels| maximum < channels)
    {
        output = Some(maximum);
    }
    output
}

/// Normalize segmented-stream channel layouts to Jellyfin's HLS-compatible
/// 1, 2, 6, or 8 channel outputs after the source and encoder caps apply.
pub(crate) fn normalize_hls_audio_channels(channels: Option<i32>) -> Option<i32> {
    channels.map(|channels| match channels {
        3 | 4 => 2,
        5 => 6,
        7 => 8,
        _ => channels,
    })
}

/// Derive the lossy audio bitrate used by official Jellyfin after channel
/// selection. Lossless and copied audio do not receive a bitrate argument.
pub(crate) fn output_audio_bitrate(
    codec: &str,
    has_source: bool,
    source_channels: Option<i32>,
    output_channels: Option<i32>,
    requested_bitrate: Option<i64>,
) -> Option<i64> {
    if !has_source || is_copy_audio_codec(codec) || is_lossless_audio_codec(codec) {
        return None;
    }
    let input_channels = source_channels.unwrap_or(0);
    let output_channels_value = output_channels.unwrap_or(0);
    let requested = requested_bitrate.unwrap_or(i64::MAX);
    let codec = codec.to_ascii_lowercase();
    let bitrate = if matches!(
        codec.as_str(),
        "" | "aac" | "mp3" | "opus" | "vorbis" | "ac3" | "eac3"
    ) {
        match (input_channels, output_channels_value) {
            (6.., 6.. | 0) => requested.min(640_000),
            (1.., 1..) => requested.min(i64::from(output_channels_value) * 128_000),
            (1.., _) => requested.min(i64::from(input_channels) * 128_000),
            _ => requested.min(384_000),
        }
    } else if matches!(codec.as_str(), "dts" | "dca") {
        match (input_channels, output_channels_value) {
            (6.., 6.. | 0) => requested.min(768_000),
            (1.., 1..) => requested.min(i64::from(output_channels_value) * 136_000),
            (1.., _) => requested.min(i64::from(input_channels) * 136_000),
            _ => requested.min(672_000),
        }
    } else {
        i64::from(output_channels.or(source_channels).unwrap_or(2)) * 128_000
    };
    Some(bitrate)
}

pub(crate) fn is_lossless_audio_codec(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "alac" | "ape" | "flac" | "mlp" | "truehd" | "wavpack"
    )
}

fn is_copy_audio_codec(codec: &str) -> bool {
    codec.eq_ignore_ascii_case("copy")
}

fn audio_bitrate_index(command: &FfmpegCommand) -> Option<usize> {
    command
        .arguments
        .windows(2)
        .position(|pair| matches!(pair[0].as_str(), "-b:a" | "-ab"))
}

fn insert_before_audio_bitrate(command: &mut FfmpegCommand, arguments: [String; 2]) {
    if let Some(index) = audio_bitrate_index(command) {
        command.arguments.splice(index..index, arguments);
    }
}

fn replace_audio_bitrate(
    command: &mut FfmpegCommand,
    expected_bitrate: i64,
    replacement: Vec<String>,
) {
    let Some(index) = audio_bitrate_index(command) else {
        return;
    };
    if command.arguments[index + 1] != expected_bitrate.to_string() {
        return;
    }
    command.arguments.splice(index..=index + 1, replacement);
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use jellyfin_controller::{FfmpegCommand, audio_command};

    use super::{
        apply_audio_vbr, apply_thread_count, audio_vbr_enabled, normalize_hls_audio_channels,
        output_audio_bitrate, output_audio_channels,
    };

    fn audio(codec: &str, bitrate: i64, channels: i32) -> FfmpegCommand {
        audio_command(
            Path::new("/usr/bin/ffmpeg"),
            Path::new("/media/input.flac"),
            Path::new("/tmp/output.mp3"),
            codec,
            Some(bitrate),
            Some(channels),
            None,
            None,
            None,
            false,
        )
    }

    #[test]
    fn configured_threads_apply_when_request_omits_cpu_limit() {
        let mut command = FfmpegCommand {
            program: PathBuf::from("ffmpeg"),
            arguments: vec!["-i".to_owned(), "input".to_owned(), "output".to_owned()],
        };
        apply_thread_count(&mut command, None, 1);
        assert_eq!(
            &command.arguments[command.arguments.len() - 3..],
            ["-threads", "1", "output"]
        );

        let mut automatic = FfmpegCommand {
            program: PathBuf::from("ffmpeg"),
            arguments: vec!["output".to_owned()],
        };
        apply_thread_count(&mut automatic, None, -1);
        assert_eq!(automatic.arguments, ["-threads", "0", "output"]);
    }

    #[test]
    fn request_cpu_limit_overrides_configuration() {
        let mut command = FfmpegCommand {
            program: PathBuf::from("ffmpeg"),
            arguments: vec!["output".to_owned()],
        };
        apply_thread_count(&mut command, Some(0), 4);
        assert_eq!(command.arguments, ["-threads", "0", "output"]);
    }

    #[test]
    fn mp3_vbr_matches_official_quality_bands() {
        let mut command = audio("mp3", 192_000, 2);
        apply_audio_vbr(&mut command, "mp3", Some(192_000), Some(2), true);
        assert!(
            command
                .arguments
                .windows(2)
                .any(|pair| pair == ["-qscale:a", "2"])
        );
        assert!(!command.arguments.iter().any(|argument| argument == "-b:a"));
    }

    #[test]
    fn request_or_global_disable_keeps_constant_bitrate() {
        let mut command = audio("mp3", 192_000, 2);
        apply_audio_vbr(&mut command, "mp3", Some(192_000), Some(2), false);
        assert!(
            command
                .arguments
                .windows(2)
                .any(|pair| pair == ["-b:a", "192000"])
        );
    }

    #[test]
    fn audio_vbr_request_defaults_to_enabled() {
        assert!(audio_vbr_enabled(true, None));
        assert!(audio_vbr_enabled(true, Some(true)));
        assert!(!audio_vbr_enabled(true, Some(false)));
        assert!(!audio_vbr_enabled(false, Some(true)));
    }

    #[test]
    fn audio_channels_follow_official_precedence_and_caps() {
        assert_eq!(
            output_audio_channels("aac", Some(6), Some(2), Some(5), Some(4), Some(3)),
            Some(2)
        );
        assert_eq!(
            output_audio_channels("aac", Some(6), None, Some(5), Some(4), Some(3)),
            Some(3)
        );
        assert_eq!(
            output_audio_channels("mp3", Some(8), None, None, None, None),
            Some(2)
        );
        assert_eq!(
            output_audio_channels("aac", Some(2), None, Some(6), None, None),
            Some(2)
        );
    }

    #[test]
    fn hls_audio_channels_use_supported_layouts() {
        assert_eq!(normalize_hls_audio_channels(Some(4)), Some(2));
        assert_eq!(normalize_hls_audio_channels(Some(5)), Some(6));
        assert_eq!(normalize_hls_audio_channels(Some(7)), Some(8));
        assert_eq!(normalize_hls_audio_channels(Some(8)), Some(8));
        assert_eq!(normalize_hls_audio_channels(None), None);
    }

    #[test]
    fn audio_bitrate_uses_official_defaults_and_caps() {
        assert_eq!(
            output_audio_bitrate("aac", true, Some(2), Some(2), None),
            Some(256_000)
        );
        assert_eq!(
            output_audio_bitrate("aac", true, Some(2), Some(2), Some(1_000_000)),
            Some(256_000)
        );
        assert_eq!(
            output_audio_bitrate("dts", true, Some(6), Some(6), None),
            Some(768_000)
        );
        assert_eq!(
            output_audio_bitrate("wmav2", true, Some(2), Some(2), Some(64_000)),
            Some(256_000)
        );
    }

    #[test]
    fn copied_lossless_or_missing_audio_has_no_bitrate() {
        for codec in ["copy", "alac", "ape", "flac", "mlp", "truehd", "wavpack"] {
            assert_eq!(
                output_audio_bitrate(codec, true, Some(2), Some(2), Some(192_000)),
                None,
                "{codec}"
            );
        }
        assert_eq!(
            output_audio_bitrate("aac", false, None, None, Some(192_000)),
            None
        );
    }
}
