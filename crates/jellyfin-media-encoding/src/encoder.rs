use std::fmt;

/// Minimum supported `FFmpeg` version.
pub const MIN_FFMPEG_VERSION: FfmpegVersion = FfmpegVersion::new(4, 4);

const MINIMUM_LIBRARY_VERSIONS: [(&str, (u32, u32)); 7] = [
    ("libavutil", (56, 70)),
    ("libavcodec", (58, 134)),
    ("libavformat", (58, 76)),
    ("libavdevice", (58, 13)),
    ("libavfilter", (7, 110)),
    ("libswscale", (5, 9)),
    ("libswresample", (3, 9)),
];

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
