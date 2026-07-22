use std::{num::ParseIntError, str::FromStr};

use jellyfin_model::MediaStream;
use thiserror::Error;

const TICKS_PER_MILLISECOND: i64 = 10_000;
const MIN_NOISE_DROP_VERSION: FfmpegVersion = FfmpegVersion::new(5, 0);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TranscodingJobType {
    #[default]
    Progressive,
    Hls,
    Dash,
}

/// `FFmpeg`'s numeric version, ordered like `System.Version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FfmpegVersion {
    major: u32,
    minor: u32,
    build: Option<u32>,
    revision: Option<u32>,
}

impl FfmpegVersion {
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self {
            major,
            minor,
            build: None,
            revision: None,
        }
    }
}

impl FromStr for FfmpegVersion {
    type Err = FfmpegVersionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let components = value
            .split('.')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()?;
        if !(2..=4).contains(&components.len()) {
            return Err(FfmpegVersionParseError::InvalidComponentCount);
        }

        Ok(Self {
            major: components[0],
            minor: components[1],
            build: components.get(2).copied(),
            revision: components.get(3).copied(),
        })
    }
}

#[derive(Debug, Error)]
pub enum FfmpegVersionParseError {
    #[error("FFmpeg version must contain two to four numeric components")]
    InvalidComponentCount,
    #[error("invalid FFmpeg version component")]
    InvalidComponent(#[from] ParseIntError),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct EncodingJobInfo {
    pub transcoding_type: TranscodingJobType,
    pub is_video_request: bool,
    pub output_video_codec: String,
    pub output_audio_codec: String,
    pub input_container: String,
    pub audio_stream: Option<MediaStream>,
    pub start_time_ticks: Option<i64>,
}

impl EncodingJobInfo {
    #[must_use]
    pub fn new(transcoding_type: TranscodingJobType) -> Self {
        Self {
            transcoding_type,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingHelper {
    encoder_version: FfmpegVersion,
}

impl EncodingHelper {
    #[must_use]
    pub const fn new(encoder_version: FfmpegVersion) -> Self {
        Self { encoder_version }
    }

    /// Builds copied-audio trimming and ADTS-to-ASC bitstream filters.
    #[must_use]
    pub fn get_audio_bit_stream_arguments(
        &self,
        state: &EncodingJobInfo,
        segment_container: &str,
        media_source_container: &str,
    ) -> String {
        let mut filters = Vec::with_capacity(2);

        if let Some(noise_filter) = self.copied_audio_trim_filter(state) {
            filters.push(noise_filter);
        }

        let segment_format = if segment_container.trim().is_empty() {
            "ts"
        } else {
            segment_container.trim_start_matches('.')
        };
        let source_uses_adts = ["ts", "aac", "hls"]
            .iter()
            .any(|container| media_source_container.eq_ignore_ascii_case(container));
        if segment_format.eq_ignore_ascii_case("mp4")
            && source_uses_adts
            && state.audio_stream.as_ref().is_some_and(is_aac)
        {
            filters.push("aac_adtstoasc".to_owned());
        }

        if filters.is_empty() {
            String::new()
        } else {
            format!(" -bsf:a {}", filters.join(","))
        }
    }

    fn copied_audio_trim_filter(&self, state: &EncodingJobInfo) -> Option<String> {
        if state.transcoding_type != TranscodingJobType::Hls
            || !state.is_video_request
            || is_copy_codec(&state.output_video_codec)
            || !is_copy_codec(&state.output_audio_codec)
            || state.input_container.eq_ignore_ascii_case("wtv")
            || self.encoder_version < MIN_NOISE_DROP_VERSION
        {
            return None;
        }

        let start_ticks = state.start_time_ticks.unwrap_or_default();
        if start_ticks <= 0 {
            return None;
        }

        let seek_seconds = format_ticks_as_seconds(start_ticks);
        Some(format!("noise=drop='lt(pts*tb\\,{seek_seconds})'"))
    }
}

fn is_copy_codec(codec: &str) -> bool {
    codec.eq_ignore_ascii_case("copy")
}

fn is_aac(stream: &MediaStream) -> bool {
    stream
        .codec
        .as_deref()
        .is_some_and(|codec| codec.to_ascii_lowercase().contains("aac"))
}

fn format_ticks_as_seconds(ticks: i64) -> String {
    let milliseconds = ticks / TICKS_PER_MILLISECOND;
    let sub_millisecond_ticks = ticks % TICKS_PER_MILLISECOND;
    let round_up = sub_millisecond_ticks > TICKS_PER_MILLISECOND / 2
        || (sub_millisecond_ticks == TICKS_PER_MILLISECOND / 2 && milliseconds % 2 != 0);
    let rounded_milliseconds = milliseconds + i64::from(round_up);
    format!(
        "{}.{:03}",
        rounded_milliseconds / 1_000,
        rounded_milliseconds % 1_000
    )
}

#[cfg(test)]
mod tests {
    use super::format_ticks_as_seconds;

    #[test]
    fn tick_formatting_matches_three_decimal_ties_to_even() {
        assert_eq!(format_ticks_as_seconds(630_630_000), "63.063");
        assert_eq!(format_ticks_as_seconds(10_005_000), "1.000");
        assert_eq!(format_ticks_as_seconds(10_015_000), "1.002");
    }
}
