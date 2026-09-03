mod external;
mod media_info;

pub use external::{
    CommandProbeProcessRunner, ExternalMediaSource, ExternalProbeError, ExternalProbeOptions,
    ExternalSourceProber, MediaProtocol, ProbeProcessOutput, ProbeProcessRequest,
    ProbeProcessRunner, external_probe_extra_arguments,
};

pub use media_info::{
    AudioSpatialFormat, MediaAttachment, MediaChapter, MediaInfo, MediaPerson, MediaPersonKind,
    MediaStream, MediaStreamFlags, MediaStreamType, ProbeContext, ProbeError, normalize_probe_file,
    normalize_probe_json,
};

/// Parses an `FFprobe` rational frame rate such as `2997/125`.
///
/// Invalid values and zero divisors return `None`.
#[must_use]
pub fn frame_rate(value: &str) -> Option<f32> {
    let (dividend, divisor) = value.split_once('/')?;
    let dividend = dividend.parse::<f32>().ok()?;
    let divisor = divisor.parse::<f32>().ok()?;
    (divisor != 0.0).then_some(dividend / divisor)
}

/// Determines whether an `FFprobe` sample aspect ratio is within one percent of
/// square pixels.
#[must_use]
pub fn is_near_square_pixel_sar(value: Option<&str>) -> bool {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return false;
    };
    let Some((numerator, denominator)) = value.split_once(':') else {
        return value == "1:1";
    };
    let Some(numerator) = numerator.parse::<f64>().ok() else {
        return value == "1:1";
    };
    let Some(denominator) = denominator.parse::<f64>().ok().filter(|value| *value > 0.0) else {
        return value == "1:1";
    };
    (numerator / denominator - 1.0).abs() <= 0.01
}

/// Estimates a typical audio bitrate when `FFprobe` omits the stream bitrate.
///
/// The estimates intentionally follow Jellyfin's conservative codec/channel
/// table. Unknown codecs or channel counts are left unset.
#[must_use]
pub fn estimated_audio_bitrate(
    codec: &str,
    profile: Option<&str>,
    channels: Option<u32>,
) -> Option<u32> {
    let channels = channels.filter(|channels| *channels > 0)?;
    if codec.is_empty() {
        return None;
    }
    let multichannel = channels > 2;
    match codec.to_ascii_lowercase().as_str() {
        "aac" | "mp3" | "mp2" => Some(if multichannel { 320_000 } else { 192_000 }),
        "ac3" | "eac3" => Some(if multichannel { 640_000 } else { 192_000 }),
        "dts" | "dca" => {
            if is_lossless_dts(profile) {
                channels.checked_mul(700_000)
            } else {
                Some(if multichannel { 1_509_000 } else { 768_000 })
            }
        }
        "opus" => Some(if multichannel { 256_000 } else { 128_000 }),
        "vorbis" => Some(if multichannel { 320_000 } else { 160_000 }),
        "wmav1" | "wmav2" | "wmapro" => Some(if multichannel { 384_000 } else { 192_000 }),
        "flac" | "alac" => channels.checked_mul(480_000),
        "truehd" | "mlp" => channels.checked_mul(700_000),
        _ => None,
    }
}

fn is_lossless_dts(profile: Option<&str>) -> bool {
    profile.is_some_and(|profile| profile.to_ascii_lowercase().contains("hd ma"))
}
