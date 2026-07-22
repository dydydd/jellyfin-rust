use std::{error::Error, fmt, time::Duration};

use jellyfin_model::{MediaProtocol, MediaStream, MediaStreamType, VideoType};

const TICKS_PER_SECOND: i64 = 10_000_000;
const MAX_RUNTIME_TICKS: i64 = 12 * 60 * 60 * TICKS_PER_SECOND;

/// Chapter metadata produced by a media probe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChapterInfo {
    pub start_position_ticks: i64,
    pub name: Option<String>,
}

/// Disc type carried by an ISO video.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsoType {
    Dvd,
    BluRay,
}

/// Video input whose probe path and protocol must be selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoProbeItem<'a> {
    pub path: &'a str,
    pub protocol: Option<MediaProtocol>,
    pub video_type: VideoType,
    pub iso_type: Option<IsoType>,
    pub is_shortcut: bool,
    pub shortcut_path: Option<&'a str>,
}

/// Exact request passed to the injected media-info capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoMediaInfoRequest<'a> {
    pub path: &'a str,
    pub protocol: MediaProtocol,
    pub video_type: VideoType,
    pub iso_type: Option<IsoType>,
    pub extract_chapters: bool,
}

/// Probe data consumed by the pure `FFProbe` video-info processor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoMediaInfo {
    pub media_streams: Vec<MediaStream>,
    pub bitrate: Option<i32>,
    pub run_time_ticks: Option<i64>,
    pub container: Option<String>,
    pub size: Option<i64>,
    pub chapters: Vec<ChapterInfo>,
}

/// Longest-playlist information returned by a Blu-ray examiner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlurayDiscInfo {
    pub media_streams: Vec<MediaStream>,
    pub run_time_ticks: Option<i64>,
    pub files: Vec<String>,
    pub playlist_name: Option<String>,
    pub chapters_seconds: Option<Vec<f64>>,
}

/// Boundary for all process and filesystem information needed by video probing.
pub trait FfprobeVideoInfoCapability {
    type Error;

    fn get_path_protocol(&self, path: &str) -> MediaProtocol;

    /// Returns the primary playable files from a DVD structure.
    ///
    /// # Errors
    ///
    /// Returns an implementation-defined error when the structure cannot be inspected.
    fn get_primary_playlist_vob_files(&self, path: &str) -> Result<Vec<String>, Self::Error>;

    fn get_bluray_info(&self, path: &str) -> Option<BlurayDiscInfo>;

    /// Returns media information for one selected file or URL.
    ///
    /// # Errors
    ///
    /// Returns an implementation-defined error when the media cannot be inspected.
    fn get_media_info(
        &self,
        request: VideoMediaInfoRequest<'_>,
    ) -> Result<VideoMediaInfo, Self::Error>;
}

/// Reason a video did not produce an ffprobe request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoProbeSkipReason {
    RemoteShortcutDisabled,
    NoPlayableDvdFiles,
    NoPlayableBlurayFiles,
}

/// Selected probe data and optional Blu-ray metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoProbeOutcome {
    pub media_info: Option<VideoMediaInfo>,
    pub bluray_info: Option<BlurayDiscInfo>,
    pub skip_reason: Option<VideoProbeSkipReason>,
}

/// Invalid runtime or chapter configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DummyChapterError {
    InvalidRuntime(i64),
    InvalidDuration(i64),
}

impl fmt::Display for DummyChapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRuntime(runtime) => write!(formatter, "invalid runtime ticks: {runtime}"),
            Self::InvalidDuration(duration) => {
                write!(formatter, "invalid dummy chapter duration: {duration}")
            }
        }
    }
}

impl Error for DummyChapterError {}

/// Pure `FFProbe` video-info processing and request selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FfprobeVideoInfo {
    dummy_chapter_duration_seconds: i64,
}

impl Default for FfprobeVideoInfo {
    fn default() -> Self {
        Self::new(5 * 60)
    }
}

impl FfprobeVideoInfo {
    #[must_use]
    pub const fn new(dummy_chapter_duration_seconds: i64) -> Self {
        Self {
            dummy_chapter_duration_seconds,
        }
    }

    /// Creates evenly spaced chapters using Jellyfin's configured duration.
    ///
    /// # Errors
    ///
    /// Returns [`DummyChapterError`] for a negative runtime, a runtime over 12 hours,
    /// or a non-positive or overflowing chapter duration.
    pub fn create_dummy_chapters(
        &self,
        run_time_ticks: Option<i64>,
    ) -> Result<Vec<ChapterInfo>, DummyChapterError> {
        let runtime = run_time_ticks.unwrap_or_default();
        if !(0..=MAX_RUNTIME_TICKS).contains(&runtime) {
            return Err(DummyChapterError::InvalidRuntime(runtime));
        }
        if runtime == 0 {
            return Ok(Vec::new());
        }
        let duration = self
            .dummy_chapter_duration_seconds
            .checked_mul(TICKS_PER_SECOND)
            .filter(|duration| *duration > 0)
            .ok_or(DummyChapterError::InvalidDuration(
                self.dummy_chapter_duration_seconds,
            ))?;
        let chapter_count = usize::try_from((runtime / duration).max(1)).unwrap_or(usize::MAX);

        Ok((0..chapter_count)
            .map(|index| ChapterInfo {
                start_position_ticks: i64::try_from(index)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(duration),
                name: None,
            })
            .collect())
    }

    /// Selects the same file, DVD, Blu-ray, and shortcut probe requests as Jellyfin.
    ///
    /// # Errors
    ///
    /// Returns the injected capability's error when DVD discovery or media inspection fails.
    pub fn probe<C: FfprobeVideoInfoCapability + ?Sized>(
        &self,
        item: VideoProbeItem<'_>,
        enable_remote_content_probe: bool,
        capability: &C,
    ) -> Result<VideoProbeOutcome, C::Error> {
        if item.is_shortcut && !enable_remote_content_probe {
            return Ok(skipped(VideoProbeSkipReason::RemoteShortcutDisabled));
        }

        match item.video_type {
            VideoType::Dvd => probe_dvd(item.path, capability),
            VideoType::BluRay => probe_bluray(item.path, capability),
            VideoType::VideoFile | VideoType::Iso => {
                let request = media_info_request(item, capability);
                capability
                    .get_media_info(request)
                    .map(|media_info| VideoProbeOutcome {
                        media_info: Some(media_info),
                        ..VideoProbeOutcome::default()
                    })
            }
        }
    }
}

fn media_info_request<'a, C: FfprobeVideoInfoCapability + ?Sized>(
    item: VideoProbeItem<'a>,
    capability: &C,
) -> VideoMediaInfoRequest<'a> {
    let (path, protocol) = if item.is_shortcut {
        let path = item.shortcut_path.unwrap_or_default();
        (path, capability.get_path_protocol(path))
    } else {
        (item.path, item.protocol.unwrap_or(MediaProtocol::File))
    };
    VideoMediaInfoRequest {
        path,
        protocol,
        video_type: item.video_type,
        iso_type: item.iso_type,
        extract_chapters: true,
    }
}

fn probe_dvd<C: FfprobeVideoInfoCapability + ?Sized>(
    path: &str,
    capability: &C,
) -> Result<VideoProbeOutcome, C::Error> {
    let files = capability.get_primary_playlist_vob_files(path)?;
    let Some((first, remaining)) = files.split_first() else {
        return Ok(skipped(VideoProbeSkipReason::NoPlayableDvdFiles));
    };
    let mut media_info = capability.get_media_info(disc_file_request(first))?;
    for file in remaining {
        let additional = capability.get_media_info(disc_file_request(file))?;
        media_info.run_time_ticks =
            add_nullable_ticks(media_info.run_time_ticks, additional.run_time_ticks);
    }
    Ok(VideoProbeOutcome {
        media_info: Some(media_info),
        ..VideoProbeOutcome::default()
    })
}

fn probe_bluray<C: FfprobeVideoInfoCapability + ?Sized>(
    path: &str,
    capability: &C,
) -> Result<VideoProbeOutcome, C::Error> {
    let Some(bluray_info) = capability.get_bluray_info(path) else {
        return Ok(skipped(VideoProbeSkipReason::NoPlayableBlurayFiles));
    };
    let Some(first) = bluray_info.files.first() else {
        return Ok(skipped(VideoProbeSkipReason::NoPlayableBlurayFiles));
    };
    let media_info = capability.get_media_info(disc_file_request(first))?;
    Ok(VideoProbeOutcome {
        media_info: Some(media_info),
        bluray_info: Some(bluray_info),
        skip_reason: None,
    })
}

fn disc_file_request(path: &str) -> VideoMediaInfoRequest<'_> {
    VideoMediaInfoRequest {
        path,
        protocol: MediaProtocol::File,
        video_type: VideoType::VideoFile,
        iso_type: None,
        extract_chapters: true,
    }
}

const fn skipped(reason: VideoProbeSkipReason) -> VideoProbeOutcome {
    VideoProbeOutcome {
        media_info: None,
        bluray_info: None,
        skip_reason: Some(reason),
    }
}

const fn add_nullable_ticks(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        _ => None,
    }
}

/// Item-level fields copied from a successful media probe.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VideoProbeMetadata {
    pub total_bitrate: Option<i32>,
    pub run_time_ticks: Option<i64>,
    pub container: Option<String>,
    pub size: Option<i64>,
}

pub fn apply_media_info_metadata(
    metadata: &mut VideoProbeMetadata,
    video_type: VideoType,
    media_info: &VideoMediaInfo,
) {
    metadata.total_bitrate = media_info.bitrate;
    metadata.run_time_ticks = media_info.run_time_ticks;
    metadata.container.clone_from(&media_info.container);
    if matches!(video_type, VideoType::BluRay | VideoType::Dvd) {
        metadata.size = media_info.size;
    }
}

/// Which embedded subtitle representation remains after probing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EmbeddedSubtitleMode {
    #[default]
    AllowAll,
    AllowText,
    AllowImage,
    AllowNone,
}

/// Final stream list and item fields derived from it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NormalizedVideoStreams {
    pub streams: Vec<MediaStream>,
    pub height: i32,
    pub width: i32,
    pub default_video_stream_index: Option<i32>,
    pub has_subtitles: bool,
    pub audio_files: Vec<String>,
    pub subtitle_files: Vec<String>,
}

#[must_use]
pub fn normalize_video_streams(
    embedded_streams: Vec<MediaStream>,
    external_audio: Vec<MediaStream>,
    external_subtitles: Vec<MediaStream>,
    subtitle_mode: EmbeddedSubtitleMode,
) -> NormalizedVideoStreams {
    let audio_files = distinct_paths(&external_audio);
    let subtitle_files = distinct_paths(&external_subtitles);
    let mut streams = external_subtitles;
    streams.extend(external_audio);
    streams.extend(embedded_streams);
    for (index, stream) in streams.iter_mut().enumerate() {
        stream.index = i32::try_from(index).unwrap_or(i32::MAX);
    }
    streams.retain(|stream| keep_stream(stream, subtitle_mode));
    let video_stream = streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Video);

    NormalizedVideoStreams {
        height: video_stream
            .and_then(|stream| stream.height)
            .unwrap_or_default(),
        width: video_stream
            .and_then(|stream| stream.width)
            .unwrap_or_default(),
        default_video_stream_index: video_stream.map(|stream| stream.index),
        has_subtitles: streams
            .iter()
            .any(|stream| stream.stream_type == MediaStreamType::Subtitle),
        streams,
        audio_files,
        subtitle_files,
    }
}

fn keep_stream(stream: &MediaStream, mode: EmbeddedSubtitleMode) -> bool {
    if stream.stream_type != MediaStreamType::Subtitle || stream.is_external {
        return true;
    }
    let is_text = stream.is_text_subtitle_stream();
    match mode {
        EmbeddedSubtitleMode::AllowAll => true,
        EmbeddedSubtitleMode::AllowText => is_text,
        EmbeddedSubtitleMode::AllowImage => !is_text,
        EmbeddedSubtitleMode::AllowNone => false,
    }
}

fn distinct_paths(streams: &[MediaStream]) -> Vec<String> {
    let mut paths = Vec::new();
    for path in streams.iter().filter_map(|stream| stream.path.as_ref()) {
        if !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    paths
}

pub fn merge_bluray_info(
    metadata: &mut VideoProbeMetadata,
    chapters: &mut Vec<ChapterInfo>,
    media_streams: &mut Vec<MediaStream>,
    bluray_info: &BlurayDiscInfo,
) {
    let ffprobe_video_stream = media_streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Video)
        .cloned();
    let mut rebuilt = media_streams
        .iter()
        .filter(|stream| stream.is_external)
        .cloned()
        .collect::<Vec<_>>();
    rebuilt.extend(bluray_info.media_streams.iter().cloned());
    for (index, stream) in rebuilt.iter_mut().enumerate() {
        stream.index = i32::try_from(index).unwrap_or(i32::MAX);
    }
    *media_streams = rebuilt;

    if let Some(runtime) = bluray_info.run_time_ticks.filter(|runtime| *runtime > 0) {
        metadata.run_time_ticks = Some(runtime);
    }
    if let Some(chapter_seconds) = &bluray_info.chapters_seconds {
        *chapters = chapter_seconds
            .iter()
            .map(|seconds| ChapterInfo {
                start_position_ticks: seconds_to_ticks(*seconds),
                name: None,
            })
            .collect();
    }

    let Some(ffprobe) = ffprobe_video_stream else {
        return;
    };
    let Some(bluray) = media_streams
        .iter_mut()
        .find(|stream| stream.stream_type == MediaStreamType::Video)
    else {
        return;
    };
    bluray.codec = ffprobe.codec;
    fill_zero(&mut bluray.bit_rate, ffprobe.bit_rate);
    fill_zero(&mut bluray.width, ffprobe.width);
    fill_zero(&mut bluray.height, ffprobe.height);
    bluray.color_transfer = ffprobe.color_transfer;
    bluray.bit_depth = ffprobe.bit_depth;
}

fn fill_zero(target: &mut Option<i32>, fallback: Option<i32>) {
    if target.unwrap_or_default() == 0 {
        *target = fallback;
    }
}

fn seconds_to_ticks(seconds: f64) -> i64 {
    let negative = seconds.is_sign_negative();
    let Ok(duration) = Duration::try_from_secs_f64(seconds.abs()) else {
        return if negative { i64::MIN } else { i64::MAX };
    };
    let ticks = i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX);
    if negative {
        ticks.saturating_neg()
    } else {
        ticks
    }
}

pub fn normalize_chapter_names(chapters: &mut [ChapterInfo], chapter_name_template: &str) {
    for (index, chapter) in chapters.iter_mut().enumerate() {
        let should_replace = chapter
            .name
            .as_deref()
            .is_none_or(|name| name.trim().is_empty() || is_time_span(name));
        if should_replace {
            chapter.name =
                Some(chapter_name_template.replace("{0}", &(index.saturating_add(1)).to_string()));
        }
    }
}

fn is_time_span(value: &str) -> bool {
    let value = value.trim().trim_start_matches(['+', '-']);
    let Some((hours, rest)) = value.split_once(':') else {
        return false;
    };
    if !valid_hours(hours) {
        return false;
    }
    let parts = rest.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        [seconds] => valid_seconds(seconds),
        [minutes, seconds] => valid_component(minutes) && valid_seconds(seconds),
        _ => false,
    }
}

fn valid_hours(value: &str) -> bool {
    let hours = value.rsplit_once('.').map_or(value, |(_, hours)| hours);
    !hours.is_empty() && hours.chars().all(|character| character.is_ascii_digit())
}

fn valid_component(value: &str) -> bool {
    value.len() == 2 && value.parse::<u8>().is_ok_and(|value| value < 60)
}

fn valid_seconds(value: &str) -> bool {
    let value = value.split_once('.').map_or(value, |(whole, _)| whole);
    valid_component(value)
}
