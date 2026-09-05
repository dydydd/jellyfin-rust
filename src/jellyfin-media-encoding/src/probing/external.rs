use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    MediaAttachment, MediaInfo, MediaStream, ProbeContext, ProbeError, normalize_probe_json,
};

/// Access protocol for a media source passed to `FFprobe`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MediaProtocol {
    #[default]
    File,
    Http,
    Rtmp,
    Rtsp,
    Udp,
    Rtp,
    Ftp,
}

/// Media source state updated with normalized probe metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalMediaSource {
    pub path: String,
    pub protocol: MediaProtocol,
    pub required_http_headers: BTreeMap<String, String>,
    pub analyze_duration_ms: Option<u64>,
    pub container: Option<String>,
    pub bitrate: Option<i64>,
    pub media_streams: Vec<MediaStream>,
    pub media_attachments: Vec<MediaAttachment>,
}

impl ExternalMediaSource {
    /// Applies normalized fields while preserving connection details such as
    /// the original path, protocol, and required headers.
    pub fn apply_media_info(&mut self, media_info: &MediaInfo) {
        self.container.clone_from(&media_info.container);
        self.bitrate = media_info.bitrate;
        self.media_streams.clone_from(&media_info.media_streams);
        self.media_attachments
            .clone_from(&media_info.media_attachments);
    }
}

/// Tunable options used to construct an external source probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalProbeOptions {
    pub extract_chapters: bool,
    pub is_audio: bool,
    pub threads: usize,
    pub configured_analyze_duration: Option<String>,
    pub configured_probe_size: Option<String>,
    pub supports_first_video_frame: bool,
}

impl Default for ExternalProbeOptions {
    fn default() -> Self {
        Self {
            extract_chapters: false,
            is_audio: false,
            threads: 1,
            configured_analyze_duration: None,
            configured_probe_size: None,
            supports_first_video_frame: false,
        }
    }
}

/// Exact, shell-free `FFprobe` invocation passed to the process boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeProcessRequest {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub source_path: String,
    pub protocol: MediaProtocol,
}

/// Captured process result containing the JSON document emitted on stdout.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProbeProcessOutput {
    pub exit_code: i32,
    pub standard_output: String,
    pub standard_error: String,
}

/// Boundary for running an external media probe.
pub trait ProbeProcessRunner {
    /// Executes one probe request.
    ///
    /// # Errors
    ///
    /// Returns an adapter error when the process cannot be started or observed.
    fn run(&self, request: &ProbeProcessRequest) -> Result<ProbeProcessOutput, String>;
}

/// Production `FFprobe` adapter. Tests should inject a fixture runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandProbeProcessRunner {
    timeout: Option<Duration>,
}

impl CommandProbeProcessRunner {
    /// Creates a process runner that terminates probes exceeding `timeout`.
    #[must_use]
    pub const fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }
}

impl ProbeProcessRunner for CommandProbeProcessRunner {
    fn run(&self, request: &ProbeProcessRequest) -> Result<ProbeProcessOutput, String> {
        let mut child = Command::new(&request.program)
            .args(&request.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "media probe stdout pipe is unavailable".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "media probe stderr pipe is unavailable".to_owned())?;
        let stdout_reader = thread::spawn(move || read_process_pipe(stdout));
        let stderr_reader = thread::spawn(move || read_process_pipe(stderr));
        let status = match wait_for_probe(&mut child, self.timeout) {
            Ok(status) => status,
            Err(error) => {
                let _ = join_process_pipe(stdout_reader, "stdout");
                let _ = join_process_pipe(stderr_reader, "stderr");
                return Err(error);
            }
        };
        let standard_output = join_process_pipe(stdout_reader, "stdout")?;
        let standard_error = join_process_pipe(stderr_reader, "stderr")?;
        Ok(ProbeProcessOutput {
            exit_code: status.code().unwrap_or(-1),
            standard_output: String::from_utf8_lossy(&standard_output).into_owned(),
            standard_error: String::from_utf8_lossy(&standard_error).into_owned(),
        })
    }
}

fn read_process_pipe(mut pipe: impl Read) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    pipe.read_to_end(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

fn join_process_pipe(
    reader: thread::JoinHandle<Result<Vec<u8>, String>>,
    name: &str,
) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| format!("media probe {name} reader panicked"))?
}

fn wait_for_probe(child: &mut Child, timeout: Option<Duration>) -> Result<ExitStatus, String> {
    let Some(timeout) = timeout else {
        return child.wait().map_err(|error| error.to_string());
    };
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| "media probe timeout is too large".to_owned())?;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "media probe timed out after {} ms",
                timeout.as_millis()
            ));
        }
        thread::sleep((deadline - now).min(Duration::from_millis(10)));
    }
}

/// Failure to run or normalize an external source probe.
#[derive(Debug)]
pub enum ExternalProbeError {
    InvalidSourcePath,
    AnalyzeDurationOverflow(u64),
    ProcessStart(String),
    ProcessFailed {
        source_path: String,
        exit_code: i32,
        standard_error: String,
    },
    Normalize(ProbeError),
}

impl fmt::Display for ExternalProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSourcePath => formatter.write_str("external probe source path is empty"),
            Self::AnalyzeDurationOverflow(value) => {
                write!(
                    formatter,
                    "analyze duration {value}ms overflows microseconds"
                )
            }
            Self::ProcessStart(error) => write!(formatter, "failed to start media probe: {error}"),
            Self::ProcessFailed {
                source_path,
                exit_code,
                standard_error,
            } => write!(
                formatter,
                "media probe failed for {source_path} with exit code {exit_code}: {standard_error}"
            ),
            Self::Normalize(error) => write!(formatter, "failed to normalize media probe: {error}"),
        }
    }
}

impl Error for ExternalProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Normalize(error) => Some(error),
            Self::InvalidSourcePath
            | Self::AnalyzeDurationOverflow(_)
            | Self::ProcessStart(_)
            | Self::ProcessFailed { .. } => None,
        }
    }
}

impl From<ProbeError> for ExternalProbeError {
    fn from(error: ProbeError) -> Self {
        Self::Normalize(error)
    }
}

/// Builds protocol-specific options prepended before the `FFprobe` input.
///
/// # Errors
///
/// Returns an overflow error when milliseconds cannot be represented as
/// microseconds for `-analyzeduration`.
pub fn external_probe_extra_arguments(
    media_source: &ExternalMediaSource,
    options: &ExternalProbeOptions,
) -> Result<Vec<String>, ExternalProbeError> {
    let mut arguments = Vec::new();
    if let Some(milliseconds) = media_source.analyze_duration_ms.filter(|value| *value > 0) {
        let microseconds = milliseconds
            .checked_mul(1_000)
            .ok_or(ExternalProbeError::AnalyzeDurationOverflow(milliseconds))?;
        arguments.extend(["-analyzeduration".to_owned(), microseconds.to_string()]);
    } else if let Some(value) = options
        .configured_analyze_duration
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        arguments.extend(["-analyzeduration".to_owned(), value.to_owned()]);
    }
    if let Some(value) = options
        .configured_probe_size
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        arguments.extend(["-probesize".to_owned(), value.to_owned()]);
    }
    if let Some(user_agent) = media_source.required_http_headers.get("User-Agent") {
        arguments.extend(["-user_agent".to_owned(), user_agent.clone()]);
    }
    if media_source.protocol == MediaProtocol::Rtsp {
        arguments.extend([
            "-rtsp_transport".to_owned(),
            "tcp+udp".to_owned(),
            "-rtsp_flags".to_owned(),
            "prefer_tcp".to_owned(),
        ]);
    }
    Ok(arguments)
}

/// External source prober using an injected process adapter.
#[derive(Clone, Debug)]
pub struct ExternalSourceProber<R> {
    probe_path: PathBuf,
    runner: R,
}

impl<R: ProbeProcessRunner> ExternalSourceProber<R> {
    #[must_use]
    pub fn new(probe_path: impl Into<PathBuf>, runner: R) -> Self {
        Self {
            probe_path: probe_path.into(),
            runner,
        }
    }

    /// Probes and normalizes a source without mutating it.
    ///
    /// # Errors
    ///
    /// Returns request construction, process, or normalization errors.
    pub fn probe(
        &self,
        media_source: &ExternalMediaSource,
        options: &ExternalProbeOptions,
    ) -> Result<MediaInfo, ExternalProbeError> {
        if media_source.path.trim().is_empty() {
            return Err(ExternalProbeError::InvalidSourcePath);
        }
        let request = self.process_request(media_source, options)?;
        let output = self
            .runner
            .run(&request)
            .map_err(ExternalProbeError::ProcessStart)?;
        if output.exit_code != 0 {
            return Err(ExternalProbeError::ProcessFailed {
                source_path: request.source_path,
                exit_code: output.exit_code,
                standard_error: output.standard_error,
            });
        }
        normalize_probe_json(
            &output.standard_output,
            ProbeContext {
                path: &media_source.path,
                is_audio: options.is_audio,
            },
        )
        .map_err(ExternalProbeError::from)
    }

    /// Probes a source and applies normalized stream/container metadata to it.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::probe`]. The source is unchanged on failure.
    pub fn probe_and_apply(
        &self,
        media_source: &mut ExternalMediaSource,
        options: &ExternalProbeOptions,
    ) -> Result<MediaInfo, ExternalProbeError> {
        let media_info = self.probe(media_source, options)?;
        media_source.apply_media_info(&media_info);
        Ok(media_info)
    }

    fn process_request(
        &self,
        media_source: &ExternalMediaSource,
        options: &ExternalProbeOptions,
    ) -> Result<ProbeProcessRequest, ExternalProbeError> {
        let mut arguments = external_probe_extra_arguments(media_source, options)?;
        arguments.extend([
            "-i".to_owned(),
            input_argument(media_source),
            "-threads".to_owned(),
            options.threads.to_string(),
            "-v".to_owned(),
            "warning".to_owned(),
            "-print_format".to_owned(),
            "json".to_owned(),
            "-show_streams".to_owned(),
        ]);
        if options.extract_chapters {
            arguments.push("-show_chapters".to_owned());
        }
        arguments.push("-show_format".to_owned());
        if media_source.protocol == MediaProtocol::File
            && !options.is_audio
            && options.supports_first_video_frame
        {
            arguments.extend(["-show_frames".to_owned(), "-only_first_vframe".to_owned()]);
        }
        Ok(ProbeProcessRequest {
            program: self.probe_path.clone(),
            arguments,
            source_path: media_source.path.clone(),
            protocol: media_source.protocol,
        })
    }
}

fn input_argument(media_source: &ExternalMediaSource) -> String {
    if media_source.protocol == MediaProtocol::File && !media_source.path.contains("://") {
        format!("file:{}", media_source.path)
    } else {
        // ALLOW: the process argument must own a path borrowed from the media source.
        media_source.path.clone()
    }
}
