use std::error::Error;
use std::fmt::{self, Write};
use std::path::Path;
use std::time::Duration;

use jellyfin_media_encoding_keyframes::KeyframeData;
use uuid::Uuid;

const TICKS_PER_MILLISECOND: i64 = 10_000;
const TICKS_PER_SECOND: u64 = 10_000_000;

/// Inputs used to generate a dynamic HLS media playlist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateMainPlaylistRequest {
    pub media_source_id: Option<Uuid>,
    pub file_path: String,
    pub desired_segment_length_ms: i32,
    pub total_runtime_ticks: i64,
    pub segment_container: String,
    pub endpoint_prefix: String,
    pub query_string: String,
    pub is_remuxing_video: bool,
}

/// Invalid input encountered while computing a playlist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HlsPlaylistError {
    InvalidSegmentParameters {
        desired_segment_length_ms: i32,
        total_runtime_ticks: i64,
    },
    SegmentLengthOverflow(i32),
    SegmentCountOverflow(i64),
    Formatting,
}

impl fmt::Display for HlsPlaylistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSegmentParameters {
                desired_segment_length_ms,
                total_runtime_ticks,
            } => write!(
                formatter,
                "invalid segment length ({desired_segment_length_ms}) or runtime ticks ({total_runtime_ticks})"
            ),
            Self::SegmentLengthOverflow(length) => {
                write!(
                    formatter,
                    "segment length in milliseconds overflows ticks: {length}"
                )
            }
            Self::SegmentCountOverflow(count) => {
                write!(formatter, "segment count cannot be represented: {count}")
            }
            Self::Formatting => formatter.write_str("failed to format HLS playlist"),
        }
    }
}

impl Error for HlsPlaylistError {}

/// Computes segment durations using the first keyframe at or after each
/// desired cut time.
///
/// This preserves Jellyfin's cumulative desired-cut schedule: a late keyframe
/// does not shift subsequent desired cut times.
#[must_use]
pub fn compute_segments(keyframe_data: &KeyframeData, desired_segment_length_ms: i32) -> Vec<f64> {
    compute_segment_ticks(keyframe_data, desired_segment_length_ms)
        .into_iter()
        .map(ticks_to_seconds)
        .collect()
}

/// Splits a runtime into equal desired lengths and one optional remainder.
///
/// # Errors
///
/// A zero runtime returns [`HlsPlaylistError::InvalidSegmentParameters`],
/// matching the official controller's invalid-operation behavior. Negative
/// runtimes and non-positive segment lengths are rejected the same way. An
/// overflow variant is returned when allocation dimensions cannot be safely
/// represented.
pub fn compute_equal_length_segments(
    desired_segment_length_ms: i32,
    total_runtime_ticks: i64,
) -> Result<Vec<f64>, HlsPlaylistError> {
    compute_equal_length_segment_ticks_inner(desired_segment_length_ms, total_runtime_ticks)
        .map(|segments| segments.into_iter().map(ticks_to_seconds).collect())
}

/// Splits a runtime into equal desired lengths and one optional remainder, in
/// ticks.
///
/// # Errors
///
/// Returns the same error cases as [`compute_equal_length_segments`].
pub fn compute_equal_length_segment_ticks(
    desired_segment_length_ms: i32,
    total_runtime_ticks: i64,
) -> Result<Vec<i64>, HlsPlaylistError> {
    compute_equal_length_segment_ticks_inner(desired_segment_length_ms, total_runtime_ticks)
}

/// Checks whether metadata keyframe extraction is allowed for a file suffix.
#[must_use]
pub fn is_extraction_allowed_for_file(file_path: &str, allowed_extensions: &[String]) -> bool {
    let Some(extension) = Path::new(file_path)
        .extension()
        .and_then(|value| value.to_str())
    else {
        return false;
    };
    allowed_extensions
        .iter()
        .any(|allowed| extension.eq_ignore_ascii_case(allowed.trim_start_matches('.')))
}

/// Creates a VOD HLS playlist. `keyframe_data` is used only for remuxing with
/// a known media source; transcodes use equal-length segments.
///
/// # Errors
///
/// Returns an [`HlsPlaylistError`] when equal-length segment inputs overflow or
/// are non-positive, or if writing to the playlist buffer fails.
pub fn create_main_playlist(
    request: &CreateMainPlaylistRequest,
    keyframe_data: Option<&KeyframeData>,
) -> Result<String, HlsPlaylistError> {
    let segment_ticks = if request.is_remuxing_video && request.media_source_id.is_some() {
        keyframe_data.map_or_else(
            || {
                compute_equal_length_segment_ticks(
                    request.desired_segment_length_ms,
                    request.total_runtime_ticks,
                )
            },
            |data| {
                Ok(compute_segment_ticks(
                    data,
                    request.desired_segment_length_ms,
                ))
            },
        )?
    } else {
        compute_equal_length_segment_ticks(
            request.desired_segment_length_ms,
            request.total_runtime_ticks,
        )?
    };

    let segment_extension = segment_file_extension(&request.segment_container);
    let is_fmp4 = segment_extension.eq_ignore_ascii_case(".mp4");
    let hls_version = if is_fmp4 { 7 } else { 3 };
    let target_duration = segment_ticks.iter().copied().max().map_or_else(
        || ceiling_milliseconds(request.desired_segment_length_ms),
        ceiling_seconds,
    );

    let mut playlist = String::with_capacity(128);
    writeln!(playlist, "#EXTM3U").map_err(|_| HlsPlaylistError::Formatting)?;
    writeln!(playlist, "#EXT-X-PLAYLIST-TYPE:VOD").map_err(|_| HlsPlaylistError::Formatting)?;
    writeln!(playlist, "#EXT-X-VERSION:{hls_version}").map_err(|_| HlsPlaylistError::Formatting)?;
    writeln!(playlist, "#EXT-X-TARGETDURATION:{target_duration}")
        .map_err(|_| HlsPlaylistError::Formatting)?;
    writeln!(playlist, "#EXT-X-MEDIA-SEQUENCE:0").map_err(|_| HlsPlaylistError::Formatting)?;

    if is_fmp4 {
        writeln!(
            playlist,
            "#EXT-X-MAP:URI=\"{}-1{}{}&runtimeTicks=0&actualSegmentLengthTicks=0\"",
            request.endpoint_prefix, segment_extension, request.query_string
        )
        .map_err(|_| HlsPlaylistError::Formatting)?;
    }

    let mut current_runtime_ticks = 0_i64;
    for (index, length_ticks) in segment_ticks.into_iter().enumerate() {
        writeln!(
            playlist,
            "#EXTINF:{:.6}, nodesc",
            ticks_to_seconds(length_ticks)
        )
        .map_err(|_| HlsPlaylistError::Formatting)?;
        writeln!(
            playlist,
            "{}{index}{}{}&runtimeTicks={current_runtime_ticks}&actualSegmentLengthTicks={length_ticks}",
            request.endpoint_prefix, segment_extension, request.query_string
        )
        .map_err(|_| HlsPlaylistError::Formatting)?;
        current_runtime_ticks = current_runtime_ticks.saturating_add(length_ticks);
    }

    writeln!(playlist, "#EXT-X-ENDLIST").map_err(|_| HlsPlaylistError::Formatting)?;
    Ok(playlist)
}

fn compute_segment_ticks(keyframe_data: &KeyframeData, desired_segment_length_ms: i32) -> Vec<i64> {
    let total_duration = keyframe_data
        .keyframe_ticks
        .last()
        .copied()
        .filter(|last| keyframe_data.total_duration < *last)
        .unwrap_or(keyframe_data.total_duration);
    let desired_segment_length_ticks =
        i64::from(desired_segment_length_ms).saturating_mul(TICKS_PER_MILLISECOND);
    let mut desired_cut_time = desired_segment_length_ticks;
    let mut last_keyframe = 0_i64;
    let mut segments = Vec::new();

    for &keyframe in &keyframe_data.keyframe_ticks {
        if keyframe >= desired_cut_time {
            segments.push(keyframe - last_keyframe);
            last_keyframe = keyframe;
            desired_cut_time = desired_cut_time.saturating_add(desired_segment_length_ticks);
        }
    }

    let remaining = total_duration - last_keyframe;
    if remaining > 0 {
        segments.push(remaining);
    }
    segments
}

fn compute_equal_length_segment_ticks_inner(
    desired_segment_length_ms: i32,
    total_runtime_ticks: i64,
) -> Result<Vec<i64>, HlsPlaylistError> {
    if desired_segment_length_ms <= 0 || total_runtime_ticks <= 0 {
        return Err(HlsPlaylistError::InvalidSegmentParameters {
            desired_segment_length_ms,
            total_runtime_ticks,
        });
    }
    let segment_length_ticks = i64::from(desired_segment_length_ms)
        .checked_mul(TICKS_PER_MILLISECOND)
        .ok_or(HlsPlaylistError::SegmentLengthOverflow(
            desired_segment_length_ms,
        ))?;
    let whole_segments = total_runtime_ticks / segment_length_ticks;
    let remaining_ticks = total_runtime_ticks % segment_length_ticks;
    let segment_count = whole_segments + i64::from(remaining_ticks != 0);
    let capacity = usize::try_from(segment_count)
        .map_err(|_| HlsPlaylistError::SegmentCountOverflow(segment_count))?;
    let mut segments = vec![segment_length_ticks; capacity];
    if remaining_ticks != 0
        && let Some(last) = segments.last_mut()
    {
        *last = remaining_ticks;
    }
    Ok(segments)
}

fn ticks_to_seconds(ticks: i64) -> f64 {
    let magnitude = ticks.unsigned_abs();
    let seconds = magnitude / TICKS_PER_SECOND;
    let nanos = (magnitude % TICKS_PER_SECOND) * 100;
    let duration = Duration::new(
        seconds,
        u32::try_from(nanos).expect("subsecond ticks always fit into nanoseconds"),
    );
    if ticks < 0 {
        -duration.as_secs_f64()
    } else {
        duration.as_secs_f64()
    }
}

const fn ceiling_seconds(ticks: i64) -> i64 {
    if ticks <= 0 {
        0
    } else {
        (ticks - 1) / 10_000_000 + 1
    }
}

const fn ceiling_milliseconds(milliseconds: i32) -> i64 {
    (milliseconds as i64 + 999) / 1_000
}

fn segment_file_extension(container: &str) -> String {
    if container.trim().is_empty() {
        ".ts".to_owned()
    } else {
        format!(".{container}")
    }
}
