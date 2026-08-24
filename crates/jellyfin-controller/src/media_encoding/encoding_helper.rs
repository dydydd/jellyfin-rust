use std::{fmt::Write as _, num::ParseIntError, path::Path, str::FromStr};

use jellyfin_media_encoding::encoder::EncoderCapabilities;
use jellyfin_model::{MediaSourceInfo, MediaStream, SubtitleDeliveryMethod};
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
    pub is_input_video: bool,
    pub output_video_codec: String,
    pub output_audio_codec: String,
    pub output_audio_sample_rate: Option<i32>,
    pub input_container: String,
    pub media_path: Option<String>,
    pub media_source: MediaSourceInfo,
    pub video_stream: Option<MediaStream>,
    pub audio_stream: Option<MediaStream>,
    pub subtitle_stream: Option<MediaStream>,
    pub subtitle_delivery_method: SubtitleDeliveryMethod,
    pub start_time_ticks: Option<i64>,
    pub run_time_ticks: Option<i64>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingHelper {
    encoder_version: FfmpegVersion,
    capabilities: EncoderCapabilities,
}

impl EncodingHelper {
    #[must_use]
    pub const fn new(encoder_version: FfmpegVersion) -> Self {
        Self {
            encoder_version,
            capabilities: EncoderCapabilities {
                version: None,
                supported: true,
                encoders: Vec::new(),
                decoders: Vec::new(),
                hwaccels: Vec::new(),
                filters: Vec::new(),
            },
        }
    }

    #[must_use]
    pub fn with_capabilities(mut self, capabilities: EncoderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn supports_encoder(&self, name: &str) -> bool {
        self.capabilities.has_encoder(name)
    }

    #[must_use]
    pub fn supports_decoder(&self, name: &str) -> bool {
        self.capabilities.has_decoder(name)
    }

    #[must_use]
    pub fn supports_filter(&self, name: &str) -> bool {
        self.capabilities.has_filter(name)
    }

    #[must_use]
    pub fn supports_hwaccel(&self, name: &str) -> bool {
        self.capabilities.has_hwaccel(name)
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

    /// Builds explicit stream mappings using `FFmpeg` input and in-file indexes.
    #[must_use]
    pub fn get_map_args(&self, state: &EncodingJobInfo) -> String {
        if state.video_stream.is_none() && state.audio_stream.is_none() {
            return if state.is_input_video {
                "-sn".to_owned()
            } else {
                String::new()
            };
        }

        if state
            .video_stream
            .as_ref()
            .is_some_and(|stream| stream.index == -1)
        {
            return "-sn".to_owned();
        }
        if state
            .audio_stream
            .as_ref()
            .is_some_and(|stream| stream.index == -1)
        {
            return if state.is_input_video {
                "-sn".to_owned()
            } else {
                String::new()
            };
        }

        let mut args = match state.video_stream.as_ref() {
            Some(stream) => format!(
                "-map 0:{}",
                find_stream_index(&state.media_source.media_streams, stream)
            ),
            None => "-vn".to_owned(),
        };

        match state.audio_stream.as_ref() {
            Some(stream) if stream.is_external => {
                let input_index = if needs_external_subtitle_muxing(state) {
                    2
                } else {
                    1
                };
                let _ = write!(
                    args,
                    " -map {input_index}:{}",
                    find_stream_index(&state.media_source.media_streams, stream)
                );
            }
            Some(stream) => {
                let _ = write!(
                    args,
                    " -map 0:{}",
                    find_stream_index(&state.media_source.media_streams, stream)
                );
            }
            None => args.push_str(" -map -0:a"),
        }

        match state.subtitle_stream.as_ref() {
            None => args.push_str(" -map -0:s"),
            Some(_) if state.subtitle_delivery_method == SubtitleDeliveryMethod::Hls => {
                args.push_str(" -map -0:s");
            }
            Some(stream) if state.subtitle_delivery_method == SubtitleDeliveryMethod::Embed => {
                let (input_index, stream_index) = if stream.is_external {
                    (1, external_subtitle_stream_index(state, stream))
                } else {
                    (
                        0,
                        find_stream_index(&state.media_source.media_streams, stream),
                    )
                };
                let _ = write!(args, " -map {input_index}:{stream_index}");
            }
            Some(stream) if stream.is_external && !stream.is_text_subtitle_stream() => {
                let _ = write!(
                    args,
                    " -map 1:{} -sn",
                    find_stream_index(&state.media_source.media_streams, stream)
                );
            }
            Some(_) => {}
        }

        args
    }

    /// Builds input arguments without starting `FFmpeg`.
    #[must_use]
    pub fn get_input_argument(&self, state: &EncodingJobInfo) -> String {
        let mut inputs = Vec::with_capacity(2);
        if let Some(path) = state.media_path.as_deref() {
            inputs.push(format!("-i {}", quote_path(path)));
        }

        if needs_external_subtitle_muxing(state)
            && let Some(subtitle_path) = state
                .subtitle_stream
                .as_ref()
                .and_then(|stream| stream.path.as_deref())
        {
            let selected_path = preferred_vobsub_path(subtitle_path);
            inputs.push(format!("-i file:{}", quote_path(&selected_path)));
        }

        inputs.join(" ")
    }

    /// Builds the progressive-audio command line without invoking `FFmpeg`.
    #[must_use]
    pub fn get_progressive_audio_full_command_line(
        &self,
        state: &EncodingJobInfo,
        output_path: &str,
    ) -> String {
        let mut arguments = Vec::new();
        let input = self.get_input_argument(state);
        if !input.is_empty() {
            arguments.push(input);
        }
        arguments.push("-threads 0 -vn".to_owned());

        if !state.output_audio_codec.is_empty() {
            arguments.push(format!(
                "-acodec {}",
                audio_encoder(&state.output_audio_codec)
            ));
        }
        if let Some(sample_rate) = state.output_audio_sample_rate {
            arguments.push(format!(
                "-ar {}",
                output_sample_rate(&state.output_audio_codec, sample_rate)
            ));
        }

        arguments.push("-id3v2_version 3 -write_id3v1 1 -y".to_owned());
        arguments.push(quote_path(output_path));
        arguments.join(" ")
    }

    /// Infers the output audio codec from a container name.
    #[must_use]
    pub fn infer_audio_codec(&self, container: &str) -> String {
        match container.trim().to_ascii_lowercase().as_str() {
            "" => "aac".to_owned(),
            "ogg" | "oga" | "ogv" | "webm" | "webma" => "opus".to_owned(),
            "m4a" | "m4b" | "mp4" | "mov" | "mkv" | "mka" => "aac".to_owned(),
            "ts" | "avi" | "flv" | "f4v" | "swf" => "mp3".to_owned(),
            value => value.to_owned(),
        }
    }

    /// Infers the output video codec from a URL or path extension.
    #[must_use]
    pub fn infer_video_codec(&self, url: &str) -> String {
        let extension = Path::new(url)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "asf" => "wmv".to_owned(),
            "webm" => "vp8".to_owned(),
            "ogg" | "ogv" => "theora".to_owned(),
            "m3u8" | "ts" => "h264".to_owned(),
            _ => "copy".to_owned(),
        }
    }

    /// Builds the HDR input color-property filter used before tonemapping.
    #[must_use]
    pub fn get_input_hdr_param(&self, color_transfer: Option<&str>) -> String {
        if color_transfer.is_some_and(|value| value.eq_ignore_ascii_case("arib-std-b67")) {
            "setparams=color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc".to_owned()
        } else {
            "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc".to_owned()
        }
    }

    /// Builds the SDR output color-property filter.
    #[must_use]
    pub fn get_output_sdr_param(&self, tonemapping_range: Option<&str>) -> String {
        let range = tonemapping_range.map_or("", |value| {
            if value.eq_ignore_ascii_case("tv") {
                ":range=tv"
            } else if value.eq_ignore_ascii_case("pc") {
                ":range=pc"
            } else {
                ""
            }
        });
        format!("setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709{range}")
    }

    /// Returns the video stream bit depth with Jellyfin's pixel-format fallback.
    #[must_use]
    pub fn get_video_color_bit_depth(&self, state: &EncodingJobInfo) -> i32 {
        let Some(stream) = state.video_stream.as_ref() else {
            return 8;
        };
        stream.bit_depth.unwrap_or_else(|| {
            let pixel_format = stream.pixel_format.as_deref().unwrap_or_default();
            if pixel_format.contains("12") {
                12
            } else if pixel_format.contains("10") || pixel_format.contains("p010") {
                10
            } else {
                8
            }
        })
    }

    /// Builds copied-video bitstream filters for H.264, HEVC, and AV1 streams.
    #[must_use]
    pub fn get_video_bit_stream_arguments(&self, state: &EncodingJobInfo) -> Option<String> {
        let stream = state.video_stream.as_ref()?;
        let codec = stream.codec.as_deref()?;
        if codec.eq_ignore_ascii_case("h264") {
            Some("-bsf:v h264_mp4toannexb".to_owned())
        } else if codec.eq_ignore_ascii_case("hevc") || codec.eq_ignore_ascii_case("h265") {
            Some("-bsf:v hevc_mp4toannexb".to_owned())
        } else if codec.eq_ignore_ascii_case("av1") {
            None
        } else {
            None
        }
    }

    /// Builds the fast-seek parameter with the HLS remux keyframe offset.
    #[must_use]
    pub fn get_fast_seek_command_line_parameter(
        &self,
        state: &EncodingJobInfo,
        is_hls_remuxing: bool,
    ) -> String {
        let Some(time) = state.start_time_ticks.filter(|time| *time > 0) else {
            return String::new();
        };
        let seek_ticks = if is_hls_remuxing {
            time.saturating_add(5_000_000)
        } else {
            time
        };
        let seek_ticks = state.run_time_ticks.map_or(seek_ticks, |runtime| {
            seek_ticks.clamp(0, runtime.saturating_sub(50_000_000).max(0))
        });
        format!("-ss {}", format_ticks_as_seconds(seek_ticks))
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

fn find_stream_index(media_streams: &[MediaStream], stream_to_find: &MediaStream) -> i32 {
    let mut index = 0;
    for stream in media_streams {
        if stream == stream_to_find {
            return index;
        }
        if stream.path == stream_to_find.path {
            index += 1;
        }
    }
    -1
}

fn external_subtitle_stream_index(state: &EncodingJobInfo, selected: &MediaStream) -> i32 {
    let mut index = 0;
    for stream in &state.media_source.media_streams {
        if stream.path == selected.path {
            if stream.index == selected.index {
                break;
            }
            index += 1;
        }
    }
    index
}

fn needs_external_subtitle_muxing(state: &EncodingJobInfo) -> bool {
    state.subtitle_stream.as_ref().is_some_and(|stream| {
        stream.is_external
            && (state.subtitle_delivery_method == SubtitleDeliveryMethod::Embed
                || (state.subtitle_delivery_method == SubtitleDeliveryMethod::Encode
                    && !stream.is_text_subtitle_stream()))
    })
}

fn preferred_vobsub_path(path: &str) -> String {
    let subtitle_path = Path::new(path);
    if subtitle_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sub"))
    {
        let index_path = subtitle_path.with_extension("idx");
        if index_path.exists() {
            return index_path.to_string_lossy().into_owned();
        }
    }
    path.to_owned()
}

fn quote_path(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

fn audio_encoder(codec: &str) -> &str {
    if codec.eq_ignore_ascii_case("opus") {
        "libopus"
    } else {
        codec
    }
}

fn output_sample_rate(codec: &str, requested: i32) -> i32 {
    if !codec.eq_ignore_ascii_case("opus") {
        return requested;
    }

    if requested <= 8_000 {
        8_000
    } else if requested <= 12_000 {
        12_000
    } else if requested <= 16_000 {
        16_000
    } else if requested <= 24_000 {
        24_000
    } else {
        48_000
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
    use jellyfin_model::MediaStream;

    use super::{
        EncodingHelper, EncodingJobInfo, FfmpegVersion, TranscodingJobType, format_ticks_as_seconds,
    };

    #[test]
    fn tick_formatting_matches_three_decimal_ties_to_even() {
        assert_eq!(format_ticks_as_seconds(630_630_000), "63.063");
        assert_eq!(format_ticks_as_seconds(10_005_000), "1.000");
        assert_eq!(format_ticks_as_seconds(10_015_000), "1.002");
    }

    #[test]
    fn codec_and_color_parameter_helpers_match_official_shapes() {
        let helper = EncodingHelper::new(FfmpegVersion::new(7, 0));
        assert_eq!(helper.infer_audio_codec("webm"), "opus");
        assert_eq!(helper.infer_audio_codec("mkv"), "aac");
        assert_eq!(helper.infer_video_codec("video.webm"), "vp8");
        assert_eq!(helper.infer_video_codec("video.m3u8"), "h264");
        assert_eq!(
            helper.get_input_hdr_param(Some("smpte2084")),
            "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc"
        );
        assert_eq!(
            helper.get_output_sdr_param(Some("tv")),
            "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv"
        );
    }

    #[test]
    fn video_bitstream_and_fast_seek_follow_copy_stream_rules() {
        let helper = EncodingHelper::new(FfmpegVersion::new(7, 0));
        let state = EncodingJobInfo {
            transcoding_type: TranscodingJobType::Hls,
            video_stream: Some(MediaStream {
                codec: Some("hevc".to_owned()),
                bit_depth: Some(10),
                ..MediaStream::default()
            }),
            start_time_ticks: Some(10_000_000),
            run_time_ticks: Some(120_000_000),
            ..EncodingJobInfo::default()
        };

        assert_eq!(
            helper.get_video_bit_stream_arguments(&state).as_deref(),
            Some("-bsf:v hevc_mp4toannexb")
        );
        assert_eq!(
            helper.get_fast_seek_command_line_parameter(&state, true),
            "-ss 1.500"
        );
        assert_eq!(helper.get_video_color_bit_depth(&state), 10);
    }
}
