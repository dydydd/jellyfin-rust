//! Keyframe extraction primitives for Jellyfin media encoding.

mod ffprobe;

use serde::{Deserialize, Serialize};

pub use ffprobe::{FfprobeError, extract_keyframes, parse_ffprobe_output};

/// Keyframe information for a media stream.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct KeyframeData {
    /// Total stream duration in 100-nanosecond ticks.
    pub total_duration: i64,
    /// Presentation timestamps of keyframes in 100-nanosecond ticks.
    pub keyframe_ticks: Vec<i64>,
}

impl KeyframeData {
    #[must_use]
    pub const fn new(total_duration: i64, keyframe_ticks: Vec<i64>) -> Self {
        Self {
            total_duration,
            keyframe_ticks,
        }
    }
}
