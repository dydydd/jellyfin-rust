use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use jellyfin_media_encoding_hls::{HlsPlaylistError, compute_equal_length_segment_ticks};
use tokio::{fs, process::Command};
use uuid::Uuid;

/// Target codecs and limits for one `FFmpeg` transcode job.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TranscodeTarget {
    pub is_video: bool,
    pub hwaccel: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub video_bitrate: Option<i64>,
    pub audio_bitrate: Option<i64>,
    pub audio_channels: Option<i32>,
    pub audio_sample_rate: Option<i32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_index: Option<i32>,
    pub burn_subtitles: bool,
    pub audio_normalize: bool,
    pub tonemap_hdr: bool,
    pub max_width: Option<i32>,
    pub max_height: Option<i32>,
    pub max_framerate: Option<f32>,
    pub start_time_ticks: Option<i64>,
}

impl Default for TranscodeTarget {
    fn default() -> Self {
        Self {
            is_video: true,
            hwaccel: None,
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            video_bitrate: None,
            audio_bitrate: None,
            audio_channels: None,
            audio_sample_rate: None,
            audio_stream_index: None,
            subtitle_index: None,
            burn_subtitles: false,
            audio_normalize: false,
            tonemap_hdr: false,
            max_width: None,
            max_height: None,
            max_framerate: None,
            start_time_ticks: None,
        }
    }
}

/// Segment output settings used by both the HLS playlist and `FFmpeg`.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsSegmentSettings {
    pub container: String,
    pub segment_length_ms: i32,
    pub min_segments: i32,
}

/// A fully-formed `FFmpeg` invocation without a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegCommand {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}

/// One rendition advertised by an HLS master playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsVariant {
    pub bandwidth: u64,
    pub resolution: String,
    pub codecs: Option<String>,
    pub url: String,
}

impl HlsVariant {
    #[must_use]
    pub fn new(bandwidth: u64, resolution: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            bandwidth,
            resolution: resolution.into(),
            codecs: None,
            url: url.into(),
        }
    }

    #[must_use]
    pub fn with_codecs(mut self, codecs: impl Into<String>) -> Self {
        self.codecs = Some(codecs.into());
        self
    }
}

/// Builds the HLS command used to produce transcode segments.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn hls_command(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_prefix: &Path,
    target: &TranscodeTarget,
    settings: &HlsSegmentSettings,
) -> FfmpegCommand {
    let mut arguments = vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-y".to_owned(),
    ];
    if let Some(hwaccel) = target.hwaccel.as_deref().filter(|value| !value.is_empty()) {
        arguments.push("-hwaccel".to_owned());
        arguments.push(hwaccel.to_owned());
    }
    if let Some(start_time_ticks) = target.start_time_ticks.filter(|ticks| *ticks > 0) {
        arguments.push("-ss".to_owned());
        arguments.push(format_ticks_as_seconds(start_time_ticks));
    }
    arguments.push("-i".to_owned());
    arguments.push(input_path.to_string_lossy().into_owned());

    if target.is_video {
        match target.video_codec.as_deref() {
            Some(codec) if !codec.eq_ignore_ascii_case("copy") => {
                arguments.push("-map".to_owned());
                arguments.push("0:v:0".to_owned());
                arguments.push("-c:v".to_owned());
                arguments.push(codec.to_owned());
            }
            Some(_) => {
                arguments.push("-map".to_owned());
                arguments.push("0:v:0".to_owned());
                arguments.push("-c:v".to_owned());
                arguments.push("copy".to_owned());
            }
            None => {}
        }
    } else {
        arguments.push("-vn".to_owned());
    }

    if let Some(bitrate) = target.video_bitrate {
        arguments.push("-b:v".to_owned());
        arguments.push(bitrate.to_string());
        arguments.push("-maxrate".to_owned());
        arguments.push(bitrate.to_string());
        arguments.push("-bufsize".to_owned());
        arguments.push((bitrate.saturating_mul(2)).to_string());
    }

    if let Some(audio_codec) = target.audio_codec.as_deref() {
        arguments.push("-map".to_owned());
        arguments.push(
            target
                .audio_stream_index
                .map_or_else(|| "0:a:0".to_owned(), |index| format!("0:a:{index}")),
        );
        arguments.push("-c:a".to_owned());
        arguments.push(audio_codec.to_owned());
    } else {
        arguments.push("-an".to_owned());
    }

    if let Some(bitrate) = target.audio_bitrate {
        arguments.push("-b:a".to_owned());
        arguments.push(bitrate.to_string());
    }
    if let Some(channels) = target.audio_channels {
        arguments.push("-ac".to_owned());
        arguments.push(channels.to_string());
    }
    if let Some(sample_rate) = target.audio_sample_rate {
        arguments.push("-ar".to_owned());
        arguments.push(sample_rate.to_string());
    }
    if target.audio_normalize {
        arguments.push("-af".to_owned());
        arguments.push("loudnorm=I=-16:TP=-1.5:LRA=11".to_owned());
    }

    let mut filters = Vec::new();
    if let Some(width) = target.max_width {
        filters.push(format!("scale='min({width},iw)':-2"));
    } else if let Some(height) = target.max_height {
        filters.push(format!("scale=-2:'min({height},ih)'"));
    }
    if let Some(framerate) = target.max_framerate {
        filters.push(format!("fps={framerate}"));
    }
    if target.tonemap_hdr {
        filters.push(
            "zscale=transfer=linear:npl=100,tonemap=tonemap=hable:desat=0,zscale=transfer=bt709:primaries=bt709:matrix=bt709"
                .to_owned(),
        );
    }
    if target.burn_subtitles
        && let Some(subtitle_index) = target.subtitle_index
    {
        let escaped = input_path
            .to_string_lossy()
            .replace('\'', "\\'")
            .replace(':', "\\:");
        filters.push(format!("subtitles='{escaped}':si={subtitle_index}"));
    }
    if !filters.is_empty() {
        arguments.push("-vf".to_owned());
        arguments.push(filters.join(","));
    }

    let segment_length_seconds = (f64::from(settings.segment_length_ms) / 1_000.0)
        .max(1.0)
        .to_string();
    arguments.push("-hls_time".to_owned());
    arguments.push(segment_length_seconds);
    arguments.push("-hls_playlist_type".to_owned());
    arguments.push("vod".to_owned());
    arguments.push("-hls_list_size".to_owned());
    arguments.push("0".to_owned());
    arguments.push("-hls_segment_filename".to_owned());
    let extension = settings.container.trim_start_matches('.');
    arguments.push(format!(
        "{}%d.{}",
        output_prefix.to_string_lossy(),
        extension
    ));
    arguments.push("-hls_flags".to_owned());
    arguments.push("temp_file+independent_segments".to_owned());
    arguments.push("-f".to_owned());
    arguments.push("hls".to_owned());

    let dummy_playlist = output_prefix.with_extension("hls.m3u8");
    arguments.push(dummy_playlist.to_string_lossy().into_owned());

    FfmpegCommand {
        program: ffmpeg_path.to_path_buf(),
        arguments,
    }
}

/// Builds the progressive audio command used by Universal Audio requests.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn audio_command(
    ffmpeg_path: &Path,
    input_path: &Path,
    output_path: &Path,
    codec: &str,
    bitrate: Option<i64>,
    channels: Option<i32>,
    sample_rate: Option<i32>,
    start_time_ticks: Option<i64>,
) -> FfmpegCommand {
    let mut arguments = vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-y".to_owned(),
    ];
    if let Some(start_time_ticks) = start_time_ticks.filter(|ticks| *ticks > 0) {
        arguments.push("-ss".to_owned());
        arguments.push(format_ticks_as_seconds(start_time_ticks));
    }
    arguments.push("-i".to_owned());
    arguments.push(input_path.to_string_lossy().into_owned());
    arguments.push("-vn".to_owned());
    arguments.push("-c:a".to_owned());
    arguments.push(codec.to_owned());
    if let Some(bitrate) = bitrate {
        arguments.push("-b:a".to_owned());
        arguments.push(bitrate.to_string());
    }
    if let Some(channels) = channels {
        arguments.push("-ac".to_owned());
        arguments.push(channels.to_string());
    }
    if let Some(sample_rate) = sample_rate {
        arguments.push("-ar".to_owned());
        arguments.push(sample_rate.to_string());
    }
    arguments.push(output_path.to_string_lossy().into_owned());

    FfmpegCommand {
        program: ffmpeg_path.to_path_buf(),
        arguments,
    }
}

fn format_ticks_as_seconds(ticks: i64) -> String {
    let milliseconds = ticks / 10_000;
    let sub_millisecond_ticks = ticks % 10_000;
    let round_up = sub_millisecond_ticks >= 5_000;
    let rounded_milliseconds = milliseconds + i64::from(round_up);
    format!(
        "{}.{:03}",
        rounded_milliseconds / 1_000,
        rounded_milliseconds % 1_000
    )
}

/// A cancellable `FFmpeg` job shared between the spawning task and the API.
#[derive(Debug, Clone)]
pub struct TranscodeJobHandle {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl TranscodeJobHandle {
    #[must_use]
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    async fn cancel_and_wait(&self) {
        self.cancel.store(true, Ordering::Release);
        for _ in 0..100 {
            if !self.running() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Runs an `FFmpeg` job and reaps its process once it exits or is cancelled.
///
/// # Errors
///
/// Returns a process-spawn error when `FFmpeg` cannot be started.
pub async fn run_ffmpeg(command: &FfmpegCommand, job: &TranscodeJobHandle) -> Result<(), String> {
    let mut child = Command::new(&command.program)
        .args(&command.arguments)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", command.program.display()))?;
    job.running.store(true, Ordering::Release);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if job.cancel.load(Ordering::Acquire) {
            let _ = child.start_kill();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    job.running.store(false, Ordering::Release);
    if status.success() || job.cancel.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(format!(
            "{} exited with {status}",
            command.program.display()
        ))
    }
}

/// Runtime metadata for one active transcode job.
#[derive(Clone, Debug)]
pub struct TranscodingJobInfo {
    pub id: String,
    pub device_id: Option<String>,
    pub play_session_id: Option<String>,
    pub path: Option<String>,
    pub is_hls: bool,
    pub started_at: Instant,
    pub last_ping: Instant,
    pub is_user_paused: bool,
}

impl TranscodingJobInfo {
    fn new(id: String) -> Self {
        let now = Instant::now();
        Self {
            id,
            device_id: None,
            play_session_id: None,
            path: None,
            is_hls: false,
            started_at: now,
            last_ping: now,
            is_user_paused: false,
        }
    }
}

#[derive(Debug, Clone)]
struct TranscodeJobEntry {
    handle: TranscodeJobHandle,
    info: TranscodingJobInfo,
}

/// A registry of active transcode jobs keyed by job and playback session.
#[derive(Debug, Clone, Default)]
pub struct TranscodeJobRegistry {
    jobs: Arc<Mutex<HashMap<String, TranscodeJobEntry>>>,
    sessions: Arc<Mutex<HashMap<String, HashSet<String>>>>,
}

impl TranscodeJobRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a job and returns its shared handle.
    pub fn register(&self, job_id: impl Into<String>) -> TranscodeJobHandle {
        let job_id = job_id.into();
        let handle = TranscodeJobHandle {
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                job_id.clone(),
                TranscodeJobEntry {
                    handle: handle.clone(),
                    info: TranscodingJobInfo::new(job_id),
                },
            );
        handle
    }

    /// Registers a job and associates it with a playback session.
    pub fn register_for_session(
        &self,
        job_id: impl Into<String>,
        device_id: &str,
        play_session_id: &str,
    ) -> TranscodeJobHandle {
        let job_id = job_id.into();
        let handle = self.register(job_id.clone());
        self.associate(&job_id, device_id, play_session_id);
        handle
    }

    /// Registers an HLS job with playback-session and path metadata.
    pub fn register_for_session_with_path(
        &self,
        job_id: impl Into<String>,
        device_id: &str,
        play_session_id: &str,
        path: &str,
    ) -> TranscodeJobHandle {
        let job_id = job_id.into();
        let handle = self.register(job_id.clone());
        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&job_id)
        {
            entry.info.device_id = non_empty(device_id);
            entry.info.play_session_id = non_empty(play_session_id);
            entry.info.path = non_empty(path);
            entry.info.is_hls = true;
        }
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_key(device_id, play_session_id))
            .or_default()
            .insert(job_id);
        handle
    }

    /// Associates an already running job with a playback session.
    pub fn associate(&self, job_id: &str, device_id: &str, play_session_id: &str) {
        if device_id.trim().is_empty() || play_session_id.trim().is_empty() {
            return;
        }
        if let Some(entry) = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(job_id)
        {
            entry.info.device_id = Some(device_id.to_owned());
            entry.info.play_session_id = Some(play_session_id.to_owned());
        }
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_key(device_id, play_session_id))
            .or_default()
            .insert(job_id.to_owned());
    }

    #[must_use]
    pub fn is_running(&self, job_id: &str) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .is_some_and(|entry| entry.handle.running())
    }

    /// Updates the last-ping timestamp for every job in a playback session.
    pub fn ping(&self, play_session_id: &str, is_user_paused: Option<bool>) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values_mut()
            .filter(|entry| {
                entry
                    .info
                    .play_session_id
                    .as_deref()
                    .is_some_and(|session| session.eq_ignore_ascii_case(play_session_id))
            })
            .for_each(|entry| {
                entry.info.last_ping = Instant::now();
                if let Some(paused) = is_user_paused {
                    entry.info.is_user_paused = paused;
                }
            });
    }

    /// Returns metadata for the active job in a playback session.
    #[must_use]
    pub fn get(&self, play_session_id: &str) -> Option<TranscodingJobInfo> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .find(|entry| {
                entry
                    .info
                    .play_session_id
                    .as_deref()
                    .is_some_and(|session| session.eq_ignore_ascii_case(play_session_id))
            })
            .map(|entry| entry.info.clone())
    }

    /// Returns a snapshot of every active transcode job.
    #[must_use]
    pub fn list(&self) -> Vec<TranscodingJobInfo> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|entry| entry.info.clone())
            .collect()
    }

    /// Cancels every running job whose last ping is older than `timeout`.
    pub async fn stop_stale_jobs(&self, timeout: Duration) -> Vec<String> {
        let now = Instant::now();
        let stale = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|entry| {
                entry.handle.running() && now.duration_since(entry.info.last_ping) > timeout
            })
            .map(|entry| entry.info.id.clone())
            .collect::<Vec<_>>();
        for job_id in &stale {
            self.stop(job_id).await;
        }
        stale
    }

    /// Cancels and removes a single transcode job.
    pub async fn stop(&self, job_id: &str) {
        let handle = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .map(|entry| entry.handle.clone());
        if let Some(handle) = handle {
            handle.cancel_and_wait().await;
        }
        self.remove(job_id);
    }

    /// Cancels all jobs belonging to a playback session and returns their ids.
    pub async fn stop_for_session(&self, device_id: &str, play_session_id: &str) -> Vec<String> {
        let key = session_key(device_id, play_session_id);
        let job_ids = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key)
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        for job_id in &job_ids {
            self.stop(job_id).await;
        }
        job_ids
    }

    pub fn remove(&self, job_id: &str) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(job_id);
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

fn session_key(device_id: &str, play_session_id: &str) -> String {
    format!("{device_id}:{play_session_id}")
}

/// Generates a stable job identifier for one transcode URL.
#[must_use]
pub fn hls_job_id(
    item_id: Uuid,
    media_source_id: Option<&str>,
    start_time_ticks: Option<i64>,
    target: &TranscodeTarget,
    settings: &HlsSegmentSettings,
) -> String {
    use md5::{Digest, Md5};
    let mut digest = Md5::new();
    digest.update(item_id.as_bytes());
    digest.update(media_source_id.unwrap_or_default().as_bytes());
    digest.update(start_time_ticks.unwrap_or_default().to_le_bytes());
    digest.update(target.video_codec.as_deref().unwrap_or_default().as_bytes());
    digest.update(target.audio_codec.as_deref().unwrap_or_default().as_bytes());
    digest.update(target.video_bitrate.unwrap_or_default().to_le_bytes());
    digest.update(target.audio_bitrate.unwrap_or_default().to_le_bytes());
    digest.update(target.max_width.unwrap_or_default().to_le_bytes());
    digest.update(target.max_height.unwrap_or_default().to_le_bytes());
    digest.update(target.hwaccel.as_deref().unwrap_or_default().as_bytes());
    digest.update(target.subtitle_index.unwrap_or_default().to_le_bytes());
    digest.update([
        u8::from(target.burn_subtitles),
        u8::from(target.audio_normalize),
        u8::from(target.tonemap_hdr),
    ]);
    digest.update(settings.segment_length_ms.to_le_bytes());
    digest.update(settings.container.as_bytes());
    let bytes = digest.finalize();
    let mut job_id = String::with_capacity(17);
    for byte in bytes.iter().take(8) {
        let _ = write!(job_id, "{byte:02x}");
    }
    job_id.push('-');
    job_id.push_str(item_id.simple().to_string().get(..8).unwrap_or_default());
    job_id
}

/// Creates the official dynamic HLS main playlist served by `main.m3u8`.
///
/// # Errors
///
/// Returns an [`HlsPlaylistError`] when the segment inputs cannot be formatted.
#[allow(clippy::cast_precision_loss)]
pub fn build_main_playlist(
    item_id: Uuid,
    job_id: &str,
    runtime_ticks: Option<i64>,
    settings: &HlsSegmentSettings,
    media_type: &str,
) -> Result<String, HlsPlaylistError> {
    let endpoint_prefix = format!("/{media_type}/{item_id}/hls1/{job_id}");
    let segment_ticks = compute_equal_length_segment_ticks(
        settings.segment_length_ms,
        runtime_ticks.unwrap_or_default(),
    )?;
    let extension = if settings.container.trim().is_empty() {
        "ts"
    } else {
        settings.container.trim_start_matches('.')
    };
    let target_duration = segment_ticks
        .iter()
        .copied()
        .max()
        .map_or(1, |ticks| (ticks + 9_999_999) / 10_000_000)
        .max(1);
    let mut playlist = format!(
        "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{target_duration}\n#EXT-X-MEDIA-SEQUENCE:0\n"
    );
    let mut current_runtime_ticks = 0_i64;
    for (index, length_ticks) in segment_ticks.iter().enumerate() {
        let _ = write!(
            playlist,
            "#EXTINF:{:.6},\n{endpoint_prefix}/{index}.{extension}?runtimeTicks={current_runtime_ticks}&actualSegmentLengthTicks={length_ticks}\n",
            *length_ticks as f64 / 10_000_000.0
        );
        current_runtime_ticks = current_runtime_ticks.saturating_add(*length_ticks);
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    Ok(playlist)
}

/// Creates a single-variant master playlist for compatibility callers.
#[must_use]
pub fn build_master_playlist(main_url: &str) -> String {
    build_variant_master_playlist(&[HlsVariant::new(8_000_000, "1920x1080", main_url)])
}

/// Creates an HLS master playlist advertising one or more renditions.
#[must_use]
pub fn build_variant_master_playlist(variants: &[HlsVariant]) -> String {
    let mut playlist = "#EXTM3U\n".to_owned();
    for variant in variants {
        let codecs = variant
            .codecs
            .as_deref()
            .map_or_else(String::new, |codecs| format!(",CODECS=\"{codecs}\""));
        let _ = write!(
            playlist,
            "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}{codecs}\n{}\n",
            variant.bandwidth, variant.resolution, variant.url
        );
    }
    playlist
}

/// Duration `FFmpeg` should wait for a job's first playlist or segment.
pub const PLAYLIST_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Checks for the first generated segment before serving an HLS playlist.
///
/// # Errors
///
/// Returns an I/O error when the output directory cannot be read.
pub async fn wait_for_segment(
    transcode_directory: &Path,
    job_id: &str,
    extension: &str,
) -> Result<(), std::io::Error> {
    let deadline = tokio::time::Instant::now() + PLAYLIST_WAIT_TIMEOUT;
    loop {
        let mut entries = match fs::read_dir(transcode_directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut found = false;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(job_id) && name.ends_with(&format!(".{extension}")) {
                found = true;
                break;
            }
        }
        if found || tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hls_command_uses_segment_prefix_and_expected_codecs() {
        let command = hls_command(
            Path::new("/usr/bin/ffmpeg"),
            Path::new("/media/movie.mkv"),
            Path::new("/tmp/transcodes/job1"),
            &TranscodeTarget {
                video_codec: Some("h264".to_owned()),
                audio_codec: Some("aac".to_owned()),
                video_bitrate: Some(4_000_000),
                audio_bitrate: Some(128_000),
                ..TranscodeTarget::default()
            },
            &HlsSegmentSettings {
                container: "ts".to_owned(),
                segment_length_ms: 6_000,
                min_segments: 2,
            },
        );

        let joined = command.arguments.join(" ");
        assert!(joined.contains("-i /media/movie.mkv"));
        assert!(joined.contains("-c:v h264"));
        assert!(joined.contains("-c:a aac"));
        assert!(joined.contains("-b:v 4000000"));
        assert!(joined.contains("-b:a 128000"));
        assert!(joined.contains("-hls_time 6"));
        assert!(joined.contains("-hls_segment_filename /tmp/transcodes/job1%d.ts"));
        assert!(joined.ends_with(".hls.m3u8"));
    }

    #[test]
    fn hls_command_places_input_seek_before_the_input() {
        let command = hls_command(
            Path::new("/usr/bin/ffmpeg"),
            Path::new("/media/movie.mkv"),
            Path::new("/tmp/transcodes/job1"),
            &TranscodeTarget {
                start_time_ticks: Some(65_000_000),
                ..TranscodeTarget::default()
            },
            &HlsSegmentSettings {
                container: "ts".to_owned(),
                segment_length_ms: 6_000,
                min_segments: 2,
            },
        );

        let joined = command.arguments.join(" ");
        assert!(joined.contains("-ss 6.500 -i /media/movie.mkv"));
    }

    #[test]
    fn audio_command_encodes_only_audio_with_requested_limits() {
        let command = audio_command(
            Path::new("/usr/bin/ffmpeg"),
            Path::new("/media/song.flac"),
            Path::new("/tmp/transcodes/out.mp3"),
            "mp3",
            Some(192_000),
            Some(2),
            Some(44_100),
            None,
        );

        assert_eq!(
            command.arguments,
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                "/media/song.flac",
                "-vn",
                "-c:a",
                "mp3",
                "-b:a",
                "192000",
                "-ac",
                "2",
                "-ar",
                "44100",
                "/tmp/transcodes/out.mp3",
            ]
        );
    }

    #[test]
    fn job_ids_are_stable_and_item_specific() {
        let item = Uuid::from_u128(1);
        let target = TranscodeTarget::default();
        let settings = HlsSegmentSettings {
            container: "ts".to_owned(),
            segment_length_ms: 6_000,
            min_segments: 2,
        };
        let first = hls_job_id(item, Some("source"), Some(100), &target, &settings);
        assert_eq!(
            first,
            hls_job_id(item, Some("source"), Some(100), &target, &settings)
        );
        assert_ne!(
            first,
            hls_job_id(item, Some("other"), Some(100), &target, &settings)
        );
        assert_ne!(
            first,
            hls_job_id(
                Uuid::from_u128(2),
                Some("source"),
                Some(100),
                &target,
                &settings
            )
        );
    }

    #[test]
    fn master_playlist_points_to_the_main_playlist() {
        let master = build_master_playlist("main.m3u8?jobId=abc");
        assert!(master.contains("#EXT-X-STREAM-INF"));
        assert!(master.contains("main.m3u8?jobId=abc"));
    }

    #[test]
    fn variant_master_playlist_advertises_multiple_renditions() {
        let master = build_variant_master_playlist(&[
            HlsVariant::new(2_000_000, "640x360", "low.m3u8"),
            HlsVariant::new(5_000_000, "1280x720", "mid.m3u8").with_codecs("avc1.640028"),
            HlsVariant::new(8_000_000, "1920x1080", "high.m3u8"),
        ]);
        assert_eq!(
            master,
            "#EXTM3U\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=640x360\nlow.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1280x720,CODECS=\"avc1.640028\"\nmid.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=8000000,RESOLUTION=1920x1080\nhigh.m3u8\n"
        );
    }

    #[test]
    fn hls_command_supports_hwaccel_subtitle_burn_audio_normalization_and_hdr() {
        let command = hls_command(
            Path::new("/usr/bin/ffmpeg"),
            Path::new("/media/movie.mkv"),
            Path::new("/tmp/transcodes/job1"),
            &TranscodeTarget {
                hwaccel: Some("vaapi".to_owned()),
                subtitle_index: Some(3),
                burn_subtitles: true,
                audio_normalize: true,
                tonemap_hdr: true,
                ..TranscodeTarget::default()
            },
            &HlsSegmentSettings {
                container: "ts".to_owned(),
                segment_length_ms: 6_000,
                min_segments: 2,
            },
        );

        let joined = command.arguments.join(" ");
        assert!(joined.contains("-hwaccel vaapi"));
        assert!(joined.contains("-af loudnorm=I=-16:TP=-1.5:LRA=11"));
        assert!(joined.contains("tonemap=tonemap=hable:desat=0"));
        assert!(joined.contains("subtitles='/media/movie.mkv':si=3"));
    }

    #[test]
    fn main_playlist_uses_official_hls1_segment_contract() {
        let playlist = build_main_playlist(
            Uuid::from_u128(1),
            "job1",
            Some(600_000_000),
            &HlsSegmentSettings {
                container: "ts".to_owned(),
                segment_length_ms: 6_000,
                min_segments: 2,
            },
            "Videos",
        )
        .expect("playlist");
        assert!(playlist.contains("#EXTM3U"));
        assert!(playlist.contains("/Videos/00000000-0000-0000-0000-000000000001/hls1/job1/0.ts?"));
        assert!(playlist.contains("runtimeTicks=0&actualSegmentLengthTicks=60000000"));
    }

    #[test]
    fn registry_tracks_session_metadata_and_pings() {
        let registry = TranscodeJobRegistry::new();
        registry.register_for_session_with_path(
            "job-1",
            "device-1",
            "play-session-1",
            "/media/video.mkv",
        );

        let info = registry.get("play-session-1").expect("job metadata");
        assert_eq!(info.id, "job-1");
        assert_eq!(info.device_id.as_deref(), Some("device-1"));
        assert_eq!(info.path.as_deref(), Some("/media/video.mkv"));
        assert!(info.is_hls);

        registry.ping("play-session-1", Some(true));
        let paused = registry.get("play-session-1").expect("job metadata");
        assert!(paused.is_user_paused);
        assert_eq!(registry.list().len(), 1);
    }

    #[tokio::test]
    async fn stop_for_session_kills_running_ffmpeg_job() {
        let registry = TranscodeJobRegistry::new();
        let job = registry.register_for_session("job-1", "device-1", "play-session-1");
        let command = FfmpegCommand {
            program: PathBuf::from("sleep"),
            arguments: vec!["30".to_owned()],
        };
        let task_job = job.clone();
        let running_job = tokio::spawn(async move { run_ffmpeg(&command, &task_job).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(job.running());

        let stopped = registry
            .stop_for_session("device-1", "play-session-1")
            .await;

        assert_eq!(stopped, vec!["job-1".to_owned()]);
        assert!(running_job.await.unwrap().is_ok());
        assert!(!job.running());
        assert!(!registry.is_running("job-1"));
    }
}
