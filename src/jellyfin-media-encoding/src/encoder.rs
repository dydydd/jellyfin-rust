use std::{fmt, io, path::PathBuf, process::Command};

use thiserror::Error;

mod apple_platform;

pub use apple_platform::{
    ApplePlatformCapability, ApplePlatformError, CpuArchitecture, Platform, get_sysctl_value,
    has_apple_av1_hardware_acceleration, is_apple_silicon,
};

/// Minimum supported `FFmpeg` version.
pub const MIN_FFMPEG_VERSION: FfmpegVersion = FfmpegVersion::new(4, 4);

const MINIMUM_LIBRARY_VERSIONS: [(&str, (u32, u32)); 8] = [
    ("libavutil", (56, 70)),
    ("libavcodec", (58, 134)),
    ("libavformat", (58, 76)),
    ("libavdevice", (58, 13)),
    ("libavfilter", (7, 110)),
    ("libswscale", (5, 9)),
    ("libswresample", (3, 9)),
    ("libpostproc", (55, 9)),
];

/// Codec, hardware-acceleration, and filter capabilities reported by ffmpeg.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EncoderCapabilities {
    pub version: Option<FfmpegVersion>,
    pub supported: bool,
    pub encoders: Vec<String>,
    pub decoders: Vec<String>,
    pub hwaccels: Vec<String>,
    pub filters: Vec<String>,
}

/// Failures while probing an ffmpeg installation.
#[derive(Debug, Error)]
pub enum EncoderValidationError {
    #[error("failed to run {program}: {source}")]
    Process {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("encoder output is not UTF-8")]
    InvalidUtf8,
}

impl EncoderCapabilities {
    #[must_use]
    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    #[must_use]
    pub fn has_decoder(&self, name: &str) -> bool {
        self.decoders
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    #[must_use]
    pub fn has_filter(&self, name: &str) -> bool {
        self.filters
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    #[must_use]
    pub fn has_hwaccel(&self, name: &str) -> bool {
        self.hwaccels
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }
}

/// Validates an `ffmpeg` binary and probes its available capabilities.
#[derive(Clone, Debug)]
pub struct EncoderValidator {
    ffmpeg_path: PathBuf,
}

impl EncoderValidator {
    #[must_use]
    pub fn new(ffmpeg_path: impl Into<PathBuf>) -> Self {
        Self {
            ffmpeg_path: ffmpeg_path.into(),
        }
    }

    /// Runs `ffmpeg -version`, `-encoders`, `-decoders`, `-hwaccels`, and
    /// `-filters`, returning a capability snapshot.
    ///
    /// # Errors
    ///
    /// Returns a process or UTF-8 error when any probe fails.
    pub fn validate(&self) -> Result<EncoderCapabilities, EncoderValidationError> {
        let version_output = self.run(&["-version"])?;
        let version = ffmpeg_version(&version_output);
        let supported = is_supported_ffmpeg_version(&version_output);
        Ok(EncoderCapabilities {
            version,
            supported,
            encoders: parse_codecs(&self.run(&["-encoders"])?),
            decoders: parse_codecs(&self.run(&["-decoders"])?),
            hwaccels: parse_hwaccels(&self.run(&["-hwaccels"])?),
            filters: parse_filters(&self.run(&["-filters"])?),
        })
    }

    #[must_use]
    pub fn encoder_path(&self) -> &PathBuf {
        &self.ffmpeg_path
    }

    fn run(&self, arguments: &[&str]) -> Result<String, EncoderValidationError> {
        let program = self.ffmpeg_path.to_string_lossy().into_owned();
        let output = Command::new(&self.ffmpeg_path)
            .args(arguments)
            .output()
            .map_err(|source| EncoderValidationError::Process { program, source })?;
        String::from_utf8(output.stdout).map_err(|_| EncoderValidationError::InvalidUtf8)
    }
}

/// Encoder/probe facade used by the server runtime.
#[derive(Clone, Debug)]
pub struct MediaEncoder {
    validator: EncoderValidator,
    ffprobe_path: PathBuf,
}

impl MediaEncoder {
    #[must_use]
    pub fn new(ffmpeg_path: impl Into<PathBuf>, ffprobe_path: impl Into<PathBuf>) -> Self {
        Self {
            validator: EncoderValidator::new(ffmpeg_path),
            ffprobe_path: ffprobe_path.into(),
        }
    }

    #[must_use]
    pub fn encoder_path(&self) -> &PathBuf {
        self.validator.encoder_path()
    }

    #[must_use]
    pub fn ffprobe_path(&self) -> &PathBuf {
        &self.ffprobe_path
    }

    /// Validates the encoder and returns its capabilities.
    ///
    /// # Errors
    ///
    /// Returns a process or UTF-8 error when any probe fails.
    pub fn validate(&self) -> Result<EncoderCapabilities, EncoderValidationError> {
        self.validator.validate()
    }
}

fn parse_codecs(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            if flags.chars().count() != 6 {
                return None;
            }
            Some(parts.next()?.to_owned())
        })
        .collect()
}

fn parse_filters(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            if !(2..=3).contains(&flags.chars().count()) {
                return None;
            }
            Some(parts.next()?.to_owned())
        })
        .collect()
}

fn parse_hwaccels(output: &str) -> Vec<String> {
    output
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Parsed semantic version from `ffmpeg -version` output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfmpegVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: Option<u32>,
    pub revision: Option<u32>,
}

impl FfmpegVersion {
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self {
            major,
            minor,
            patch: None,
            revision: None,
        }
    }

    #[must_use]
    pub const fn with_patch(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch: Some(patch),
            revision: None,
        }
    }

    fn normalized(self) -> (u32, u32, u32, u32) {
        (
            self.major,
            self.minor,
            self.patch.unwrap_or(0),
            self.revision.unwrap_or(0),
        )
    }
}

impl fmt::Display for FfmpegVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)?;
        if let Some(patch) = self.patch {
            write!(formatter, ".{patch}")?;
        }
        if let Some(revision) = self.revision {
            write!(formatter, ".{revision}")?;
        }
        Ok(())
    }
}

/// Extracts an `FFmpeg` version from captured `ffmpeg -version` output.
///
/// Release builds are parsed from the first line. Git builds fall back to
/// validating all core library major/minor versions and return the minimum
/// supported version when those libraries are compatible.
#[must_use]
pub fn ffmpeg_version(output: &str) -> Option<FfmpegVersion> {
    parse_release_version(output).or_else(|| library_fallback_version(output))
}

/// Applies Jellyfin's minimum-version and Libav rejection rules to captured
/// encoder version output.
#[must_use]
pub fn is_supported_ffmpeg_version(output: &str) -> bool {
    if output.to_ascii_lowercase().contains("libav developers") {
        return false;
    }
    ffmpeg_version(output)
        .is_some_and(|version| version.normalized() >= MIN_FFMPEG_VERSION.normalized())
}

fn parse_release_version(output: &str) -> Option<FfmpegVersion> {
    let version = output.strip_prefix("ffmpeg version ")?;
    let version = version.strip_prefix('n').unwrap_or(version);
    let candidate: String = version
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    parse_version(&candidate)
}

fn parse_version(value: &str) -> Option<FfmpegVersion> {
    let components = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    match components.as_slice() {
        [major, minor] => Some(FfmpegVersion::new(*major, *minor)),
        [major, minor, patch] => Some(FfmpegVersion::with_patch(*major, *minor, *patch)),
        [major, minor, patch, revision] => Some(FfmpegVersion {
            major: *major,
            minor: *minor,
            patch: Some(*patch),
            revision: Some(*revision),
        }),
        _ => None,
    }
}

fn library_fallback_version(output: &str) -> Option<FfmpegVersion> {
    MINIMUM_LIBRARY_VERSIONS
        .iter()
        .all(|(name, minimum)| {
            find_library_version(output, name).is_some_and(|found| found >= *minimum)
        })
        .then_some(MIN_FFMPEG_VERSION)
}

fn find_library_version(output: &str, expected_name: &str) -> Option<(u32, u32)> {
    output.lines().find_map(|line| {
        let version = line.strip_prefix(expected_name)?.trim_start();
        let (major, remainder) = version.split_once('.')?;
        let minor: String = remainder
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let major = major.parse().ok()?;
        let minor = minor.parse().ok()?;
        Some((major, minor))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_parsers_read_official_ffmpeg_layouts() {
        let encoders = parse_codecs(
            "Encoders:\n V..... libx264              libx264 H.264\n A..... aac                  AAC",
        );
        assert_eq!(encoders, ["libx264", "aac"]);

        let decoders = parse_codecs(
            "Decoders:\n V....D h264                 H.264\n V....D av1                  Alliance for Open Media",
        );
        assert_eq!(decoders, ["h264", "av1"]);

        let filters = parse_filters(
            "Filters:\n T.. alphasrc              alpha source\n ..C zscale                scale image",
        );
        assert_eq!(filters, ["alphasrc", "zscale"]);

        let hwaccels = parse_hwaccels("Hardware acceleration methods:\ncuda\nvaapi");
        assert_eq!(hwaccels, ["cuda", "vaapi"]);
    }

    #[test]
    fn encoder_capabilities_report_required_names() {
        let capabilities = EncoderCapabilities {
            version: Some(FfmpegVersion::new(7, 0)),
            supported: true,
            encoders: vec!["libx264".to_owned()],
            decoders: vec!["hevc".to_owned()],
            hwaccels: vec!["vaapi".to_owned()],
            filters: vec!["zscale".to_owned()],
        };
        assert!(capabilities.has_encoder("LIBX264"));
        assert!(capabilities.has_decoder("hevc"));
        assert!(capabilities.has_hwaccel("vaapi"));
        assert!(capabilities.has_filter("zscale"));
        assert!(!capabilities.has_encoder("libx265"));
    }
}
