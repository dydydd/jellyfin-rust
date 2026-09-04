use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use serde_json::json;

use super::{SubtitleParseError, SubtitleTrack, parse_subtitle};

/// Protocol used to access a subtitle resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubtitleProtocol {
    File,
    Http,
}

/// A readable subtitle and its normalized format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleInfo {
    pub path: PathBuf,
    pub protocol: SubtitleProtocol,
    pub format: String,
    pub is_external: bool,
}

/// Media source fields needed to select and cache subtitle streams.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleMediaSource {
    pub id: String,
    pub path: PathBuf,
    pub protocol: SubtitleProtocol,
    pub cache_directory: PathBuf,
}

/// Subtitle stream fields used by the encoder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleMediaStream {
    pub index: usize,
    pub path: PathBuf,
    pub codec: String,
    pub is_external: bool,
}

/// An exact, shell-free invocation of the configured subtitle encoder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleProcessRequest {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub input_path: PathBuf,
    pub output_paths: Vec<PathBuf>,
}

/// Captured completion state from an encoder process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubtitleProcessOutput {
    pub exit_code: i32,
    pub standard_error: String,
}

/// Boundary for running external subtitle extraction or conversion.
pub trait SubtitleProcessRunner {
    /// Runs one process request.
    ///
    /// # Errors
    ///
    /// Returns a textual adapter error when the process cannot be started or observed.
    fn run(&self, request: &SubtitleProcessRequest) -> Result<SubtitleProcessOutput, String>;
}

/// Production process adapter. Tests should inject a fixture runner instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandSubtitleProcessRunner;

impl SubtitleProcessRunner for CommandSubtitleProcessRunner {
    fn run(&self, request: &SubtitleProcessRequest) -> Result<SubtitleProcessOutput, String> {
        let output = Command::new(&request.program)
            .args(&request.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| error.to_string())?;
        Ok(SubtitleProcessOutput {
            exit_code: output.status.code().unwrap_or(-1),
            standard_error: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A subtitle payload that preserves file-backed streams when no recoding is needed.
#[derive(Debug)]
pub enum SubtitleInput {
    File(File),
    Memory(Cursor<Vec<u8>>),
}

impl SubtitleInput {
    #[must_use]
    pub const fn is_file_backed(&self) -> bool {
        matches!(self, Self::File(_))
    }

    #[must_use]
    pub const fn is_memory_backed(&self) -> bool {
        matches!(self, Self::Memory(_))
    }
}

impl Read for SubtitleInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File(file) => file.read(buffer),
            Self::Memory(cursor) => cursor.read(buffer),
        }
    }
}

/// Subtitle encoding or extraction failure.
#[derive(Debug)]
pub enum SubtitleEncoderError {
    Io(io::Error),
    Parse(SubtitleParseError),
    UnsupportedProtocol(SubtitleProtocol),
    UnsupportedOutputFormat(String),
    ProcessStart(String),
    ProcessFailed {
        input_path: PathBuf,
        exit_code: i32,
        standard_error: String,
    },
    MissingProcessOutput(PathBuf),
}

impl fmt::Display for SubtitleEncoderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "subtitle I/O failed: {error}"),
            Self::Parse(error) => write!(formatter, "subtitle parsing failed: {error}"),
            Self::UnsupportedProtocol(protocol) => {
                write!(formatter, "unsupported subtitle protocol: {protocol:?}")
            }
            Self::UnsupportedOutputFormat(format) => {
                write!(formatter, "unsupported subtitle output format: {format}")
            }
            Self::ProcessStart(error) => {
                write!(formatter, "failed to start subtitle encoder: {error}")
            }
            Self::ProcessFailed {
                input_path,
                exit_code,
                standard_error,
            } => write!(
                formatter,
                "subtitle encoding failed for {} with exit code {exit_code}: {standard_error}",
                input_path.display()
            ),
            Self::MissingProcessOutput(path) => {
                write!(
                    formatter,
                    "subtitle encoder produced no output at {}",
                    path.display()
                )
            }
        }
    }
}

impl Error for SubtitleEncoderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::UnsupportedProtocol(_)
            | Self::UnsupportedOutputFormat(_)
            | Self::ProcessStart(_)
            | Self::ProcessFailed { .. }
            | Self::MissingProcessOutput(_) => None,
        }
    }
}

impl From<io::Error> for SubtitleEncoderError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SubtitleParseError> for SubtitleEncoderError {
    fn from(error: SubtitleParseError) -> Self {
        Self::Parse(error)
    }
}

/// Subtitle encoder with an injected external-process boundary.
#[derive(Clone, Debug)]
pub struct SubtitleEncoder<R> {
    encoder_path: PathBuf,
    runner: R,
}

impl<R: SubtitleProcessRunner> SubtitleEncoder<R> {
    #[must_use]
    pub fn new(encoder_path: impl Into<PathBuf>, runner: R) -> Self {
        Self {
            encoder_path: encoder_path.into(),
            runner,
        }
    }

    /// Converts one text subtitle stream without invoking the external encoder.
    ///
    /// # Errors
    ///
    /// Returns a parse error for invalid input or an unsupported-output error.
    pub fn convert_subtitles(
        &self,
        reader: impl Read,
        input_format: &str,
        output_format: &str,
        start_time_ticks: i64,
        end_time_ticks: i64,
        preserve_original_timestamps: bool,
    ) -> Result<Vec<u8>, SubtitleEncoderError> {
        convert_subtitles(
            reader,
            input_format,
            output_format,
            start_time_ticks,
            end_time_ticks,
            preserve_original_timestamps,
        )
    }

    /// Resolves an external subtitle directly or extracts/converts it into the cache.
    ///
    /// # Errors
    ///
    /// Returns an I/O or process error when a cached subtitle must be generated.
    pub fn get_readable_file(
        &self,
        media_source: &SubtitleMediaSource,
        subtitle_stream: &SubtitleMediaStream,
    ) -> Result<SubtitleInfo, SubtitleEncoderError> {
        if !subtitle_stream.is_external || has_extension(&subtitle_stream.path, "mks") {
            let format = extractable_format(&subtitle_stream.codec);
            let extension = extractable_extension(&subtitle_stream.codec);
            let output_path = cache_path(media_source, subtitle_stream.index, extension);
            let output_codec = if is_copyable_codec(&subtitle_stream.codec) {
                "copy"
            } else {
                "srt"
            };
            let request = self.extraction_request(
                &media_source.path,
                subtitle_stream.index,
                output_codec,
                &output_path,
            );
            self.run_process(request)?;
            return Ok(SubtitleInfo {
                path: output_path,
                protocol: SubtitleProtocol::File,
                is_external: is_vob_sub_format(&format),
                format,
            });
        }

        let current_format = normalized_input_format(subtitle_stream);
        if is_pgs_format(&current_format) {
            return Ok(SubtitleInfo {
                path: subtitle_stream.path.clone(),
                protocol: path_protocol(&subtitle_stream.path),
                format: "pgssub".to_owned(),
                is_external: true,
            });
        }

        if !is_parser_format(&current_format) {
            let output_path = cache_path(media_source, subtitle_stream.index, "srt");
            let request = self.conversion_request(&subtitle_stream.path, &output_path)?;
            self.run_process(request)?;
            return Ok(SubtitleInfo {
                path: output_path,
                protocol: SubtitleProtocol::File,
                format: "srt".to_owned(),
                is_external: true,
            });
        }

        Ok(SubtitleInfo {
            path: subtitle_stream.path.clone(),
            protocol: path_protocol(&subtitle_stream.path),
            format: current_format,
            is_external: true,
        })
    }

    /// Opens a subtitle and transcodes legacy local text encodings to UTF-8.
    ///
    /// UTF-8/ASCII input remains file-backed. UTF-16 and legacy single-byte
    /// or Shift-JIS encodings are returned from an owned UTF-8 buffer.
    ///
    /// # Errors
    ///
    /// Returns an I/O error for unreadable local files or an unsupported-protocol error.
    pub fn get_subtitle_stream(
        &self,
        file_info: &SubtitleInfo,
    ) -> Result<SubtitleInput, SubtitleEncoderError> {
        if file_info.protocol != SubtitleProtocol::File {
            return Err(SubtitleEncoderError::UnsupportedProtocol(
                file_info.protocol,
            ));
        }
        if file_info.is_external && is_text_format(&file_info.format) {
            let bytes = fs::read(&file_info.path)?;
            if std::str::from_utf8(strip_utf8_bom(&bytes)).is_ok() {
                return Ok(SubtitleInput::File(File::open(&file_info.path)?));
            }
            return Ok(SubtitleInput::Memory(Cursor::new(decode_to_utf8(&bytes))));
        }
        Ok(SubtitleInput::File(File::open(&file_info.path)?))
    }

    /// Returns the original stream for equivalent formats, otherwise performs
    /// an in-memory text conversion.
    ///
    /// # Errors
    ///
    /// Returns an input, parse, or unsupported-output error.
    pub fn get_subtitles(
        &self,
        file_info: &SubtitleInfo,
        output_format: &str,
        start_time_ticks: i64,
        end_time_ticks: i64,
        preserve_original_timestamps: bool,
    ) -> Result<SubtitleInput, SubtitleEncoderError> {
        let mut input = self.get_subtitle_stream(file_info)?;
        if equivalent_formats(&file_info.format, output_format) {
            return Ok(input);
        }
        let output = convert_subtitles(
            &mut input,
            &file_info.format,
            output_format,
            start_time_ticks,
            end_time_ticks,
            preserve_original_timestamps,
        )?;
        Ok(SubtitleInput::Memory(Cursor::new(output)))
    }

    fn conversion_request(
        &self,
        input_path: &Path,
        output_path: &Path,
    ) -> Result<SubtitleProcessRequest, SubtitleEncoderError> {
        let mut arguments = vec!["-nostdin".to_owned(), "-y".to_owned()];
        if let Some(charset) = detected_charset(input_path)? {
            let sami_utf16 = (has_extension(input_path, "smi")
                || has_extension(input_path, "sami"))
                && matches!(charset, "UTF-16LE" | "UTF-16BE");
            if !sami_utf16 {
                arguments.push("-sub_charenc".to_owned());
                arguments.push(charset.to_owned());
            }
        }
        arguments.extend([
            "-i".to_owned(),
            input_path.to_string_lossy().into_owned(),
            "-c:s".to_owned(),
            "srt".to_owned(),
            output_path.to_string_lossy().into_owned(),
        ]);
        Ok(SubtitleProcessRequest {
            program: self.encoder_path.clone(),
            arguments,
            input_path: input_path.to_path_buf(),
            output_paths: vec![output_path.to_path_buf()],
        })
    }

    fn extraction_request(
        &self,
        input_path: &Path,
        subtitle_stream_index: usize,
        output_codec: &str,
        output_path: &Path,
    ) -> SubtitleProcessRequest {
        SubtitleProcessRequest {
            program: self.encoder_path.clone(),
            arguments: vec![
                "-nostdin".to_owned(),
                "-y".to_owned(),
                "-i".to_owned(),
                input_path.to_string_lossy().into_owned(),
                "-copyts".to_owned(),
                "-map".to_owned(),
                format!("0:{subtitle_stream_index}"),
                "-an".to_owned(),
                "-vn".to_owned(),
                "-c:s".to_owned(),
                output_codec.to_owned(),
                output_path.to_string_lossy().into_owned(),
            ],
            input_path: input_path.to_path_buf(),
            output_paths: vec![output_path.to_path_buf()],
        }
    }

    fn run_process(&self, request: SubtitleProcessRequest) -> Result<(), SubtitleEncoderError> {
        if let Some(parent) = request.output_paths.first().and_then(|path| path.parent()) {
            fs::create_dir_all(parent)?;
        }
        let output = self
            .runner
            .run(&request)
            .map_err(SubtitleEncoderError::ProcessStart)?;
        if output.exit_code != 0 {
            remove_outputs(&request.output_paths);
            return Err(SubtitleEncoderError::ProcessFailed {
                input_path: request.input_path,
                exit_code: output.exit_code,
                standard_error: output.standard_error,
            });
        }
        for path in request.output_paths {
            if fs::metadata(&path).is_err() || fs::metadata(&path)?.len() == 0 {
                remove_outputs(std::slice::from_ref(&path));
                return Err(SubtitleEncoderError::MissingProcessOutput(path));
            }
        }
        Ok(())
    }
}

/// Converts a parsed subtitle to a supported text output format.
///
/// # Errors
///
/// Returns a parse error for invalid input or an unsupported-output error.
pub fn convert_subtitles(
    reader: impl Read,
    input_format: &str,
    output_format: &str,
    start_time_ticks: i64,
    end_time_ticks: i64,
    preserve_original_timestamps: bool,
) -> Result<Vec<u8>, SubtitleEncoderError> {
    let mut track = parse_subtitle(reader, input_format)?;
    filter_events(
        &mut track,
        start_time_ticks,
        end_time_ticks,
        preserve_original_timestamps,
    );
    let text = write_track(&track, output_format)?;
    Ok(text.into_bytes())
}

/// Applies Jellyfin's requested subtitle time window and timestamp offset.
pub fn filter_events(
    track: &mut SubtitleTrack,
    start_position_ticks: i64,
    end_time_ticks: i64,
    preserve_timestamps: bool,
) {
    track.events.retain(|event| {
        !(event.start_position_ticks - start_position_ticks < 0
            && event.end_position_ticks - start_position_ticks < 0)
            && (end_time_ticks <= 0 || event.start_position_ticks <= end_time_ticks)
    });
    if preserve_timestamps {
        return;
    }
    for event in &mut track.events {
        event.start_position_ticks = (event.start_position_ticks - start_position_ticks).max(0);
        event.end_position_ticks = (event.end_position_ticks - start_position_ticks).max(0);
    }
}

fn write_track(track: &SubtitleTrack, format: &str) -> Result<String, SubtitleEncoderError> {
    match format.to_ascii_lowercase().as_str() {
        "srt" | "subrip" => Ok(write_srt(track)),
        "vtt" | "webvtt" => Ok(write_vtt(track)),
        "ssa" => Ok(write_ssa(track, false)),
        "ass" => Ok(write_ssa(track, true)),
        "json" => serde_json::to_string(&json!({
            "title": "untitled",
            "events": track.events.iter().map(|event| json!({
                "id": event.id,
                "text": event.text,
                "startPositionTicks": event.start_position_ticks,
                "endPositionTicks": event.end_position_ticks,
            })).collect::<Vec<_>>()
        }))
        .map_err(|error| SubtitleEncoderError::UnsupportedOutputFormat(error.to_string())),
        "ttml" => Ok(write_ttml(track)),
        _ => Err(SubtitleEncoderError::UnsupportedOutputFormat(
            format.to_owned(),
        )),
    }
}

fn write_srt(track: &SubtitleTrack) -> String {
    let mut output = String::new();
    for (index, event) in track.events.iter().enumerate() {
        output.push_str(&(index + 1).to_string());
        output.push('\n');
        output.push_str(&format_srt_time(event.start_position_ticks));
        output.push_str(" --> ");
        output.push_str(&format_srt_time(event.end_position_ticks));
        output.push('\n');
        output.push_str(&event.text);
        output.push_str("\n\n");
    }
    output
}

fn write_vtt(track: &SubtitleTrack) -> String {
    let mut output = String::from("WEBVTT\n\n");
    for event in &track.events {
        output.push_str(&format_vtt_time(event.start_position_ticks));
        output.push_str(" --> ");
        output.push_str(&format_vtt_time(event.end_position_ticks));
        output.push('\n');
        output.push_str(&event.text);
        output.push_str("\n\n");
    }
    output
}

fn write_ssa(track: &SubtitleTrack, advanced: bool) -> String {
    let version = if advanced { "v4.00+" } else { "v4.00" };
    let mut output = format!(
        "[Script Info]\nTitle: untitled\nScriptType: {version}\n\n[Events]\nFormat: Layer, Start, End, Text\n"
    );
    for event in &track.events {
        output.push_str("Dialogue: 0,");
        output.push_str(&format_ssa_time(event.start_position_ticks));
        output.push(',');
        output.push_str(&format_ssa_time(event.end_position_ticks));
        output.push(',');
        output.push_str(&event.text.replace('\n', "\\N"));
        output.push('\n');
    }
    output
}

fn write_ttml(track: &SubtitleTrack) -> String {
    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<tt xmlns=\"http://www.w3.org/ns/ttml\"><body><div>\n",
    );
    for event in &track.events {
        output.push_str("<p begin=\"");
        output.push_str(&format_vtt_time(event.start_position_ticks));
        output.push_str("\" end=\"");
        output.push_str(&format_vtt_time(event.end_position_ticks));
        output.push_str("\">");
        output.push_str(&escape_xml(&event.text));
        output.push_str("</p>\n");
    }
    output.push_str("</div></body></tt>\n");
    output
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn format_srt_time(ticks: i64) -> String {
    format_time(ticks, ',')
}

fn format_vtt_time(ticks: i64) -> String {
    format_time(ticks, '.')
}

fn format_time(ticks: i64, separator: char) -> String {
    let milliseconds = ticks.max(0) / 10_000;
    let hours = milliseconds / 3_600_000;
    let minutes = milliseconds / 60_000 % 60;
    let seconds = milliseconds / 1_000 % 60;
    let fraction = milliseconds % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{fraction:03}")
}

fn format_ssa_time(ticks: i64) -> String {
    let centiseconds = ticks.max(0) / 100_000;
    let hours = centiseconds / 360_000;
    let minutes = centiseconds / 6_000 % 60;
    let seconds = centiseconds / 100 % 60;
    let fraction = centiseconds % 100;
    format!("{hours}:{minutes:02}:{seconds:02}.{fraction:02}")
}

fn normalized_input_format(stream: &SubtitleMediaStream) -> String {
    let codec = stream.codec.to_ascii_lowercase();
    if is_vob_sub_format(&codec) || is_pgs_format(&codec) {
        return codec;
    }
    let raw = stream
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .unwrap_or(&codec)
        .to_ascii_lowercase();
    match raw.as_str() {
        "subrip" => "srt".to_owned(),
        "webvtt" => "vtt".to_owned(),
        "sub" => "microdvd".to_owned(),
        _ => raw,
    }
}

fn equivalent_formats(input: &str, output: &str) -> bool {
    input.eq_ignore_ascii_case(output)
        || (input.eq_ignore_ascii_case("ssa") && output.eq_ignore_ascii_case("ass"))
        || (matches!(input.to_ascii_lowercase().as_str(), "srt" | "subrip")
            && matches!(output.to_ascii_lowercase().as_str(), "srt" | "subrip"))
        || (matches!(input.to_ascii_lowercase().as_str(), "vtt" | "webvtt")
            && matches!(output.to_ascii_lowercase().as_str(), "vtt" | "webvtt"))
}

fn cache_path(source: &SubtitleMediaSource, index: usize, extension: &str) -> PathBuf {
    let id = source
        .id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    source
        .cache_directory
        .join(format!("{id}_{index}.{extension}"))
}

fn extractable_format(codec: &str) -> String {
    if matches!(
        codec.to_ascii_lowercase().as_str(),
        "ass" | "ssa" | "pgssub"
    ) {
        codec.to_ascii_lowercase()
    } else if is_vob_sub_format(codec) {
        "mks".to_owned()
    } else {
        "srt".to_owned()
    }
}

fn extractable_extension(codec: &str) -> &str {
    if codec.eq_ignore_ascii_case("pgssub") {
        "sup"
    } else if is_vob_sub_format(codec) {
        "mks"
    } else if codec.eq_ignore_ascii_case("ass") {
        "ass"
    } else if codec.eq_ignore_ascii_case("ssa") {
        "ssa"
    } else {
        "srt"
    }
}

fn is_copyable_codec(codec: &str) -> bool {
    matches!(
        codec.to_ascii_lowercase().as_str(),
        "ass" | "ssa" | "srt" | "subrip" | "vtt" | "webvtt" | "microdvd" | "pgssub"
    ) || is_vob_sub_format(codec)
}

fn is_parser_format(format: &str) -> bool {
    matches!(
        format.to_ascii_lowercase().as_str(),
        "srt" | "subrip" | "ssa" | "ass" | "vtt" | "webvtt" | "sub" | "microdvd"
    )
}

fn is_text_format(format: &str) -> bool {
    let format = format.to_ascii_lowercase();
    format.contains("microdvd")
        || (!format.contains("pgs")
            && !format.contains("dvdsub")
            && !format.contains("vobsub")
            && !format.contains("dvbsub")
            && !matches!(format.as_str(), "sup" | "sub"))
}

fn is_pgs_format(format: &str) -> bool {
    format.to_ascii_lowercase().contains("pgs") || format.eq_ignore_ascii_case("sup")
}

fn is_vob_sub_format(format: &str) -> bool {
    let format = format.to_ascii_lowercase();
    format.contains("dvdsub") || format.contains("vobsub")
}

fn path_protocol(path: &Path) -> SubtitleProtocol {
    let path = path.to_string_lossy();
    if path.starts_with("http://") || path.starts_with("https://") {
        SubtitleProtocol::Http
    } else {
        SubtitleProtocol::File
    }
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn detected_charset(path: &Path) -> Result<Option<&'static str>, io::Error> {
    let bytes = fs::read(path)?;
    Ok(detected_charset_from_bytes(&bytes))
}

fn detected_charset_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        return Some("UTF-16LE");
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        return Some("UTF-16BE");
    }
    if std::str::from_utf8(strip_utf8_bom(bytes)).is_ok() {
        return None;
    }
    Some(guess_encoding(bytes).name())
}

fn decode_to_utf8(bytes: &[u8]) -> Vec<u8> {
    if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(bytes, true).into_bytes();
    }
    if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(bytes, false).into_bytes();
    }
    let (text, _) = guess_encoding(bytes).decode_without_bom_handling(bytes);
    text.into_owned().into_bytes()
}

fn guess_encoding(bytes: &[u8]) -> &'static Encoding {
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    detector.guess(None, Utf8Detection::Deny)
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> String {
    let words = bytes.as_chunks::<2>().0.iter().map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    char::decode_utf16(words)
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes)
}

fn remove_outputs(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}
