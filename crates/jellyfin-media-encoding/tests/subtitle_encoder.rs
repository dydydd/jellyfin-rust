use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use encoding_rs::WINDOWS_1253;
use jellyfin_media_encoding::subtitles::{
    SubtitleEncoder, SubtitleEncoderError, SubtitleInfo, SubtitleMediaSource, SubtitleMediaStream,
    SubtitleProcessOutput, SubtitleProcessRequest, SubtitleProcessRunner, SubtitleProtocol,
};

const STREAM_COUNT: usize = 8;
const CUE_COUNT: usize = 500;
const GREEK_TEXT: &str = "Καλημέρα κόσμε, αυτό είναι ένας υπότιτλος.";

#[derive(Debug)]
struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "jellyfin-rust-subtitle-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Default)]
struct FixtureState {
    requests: Mutex<Vec<SubtitleProcessRequest>>,
}

#[derive(Clone, Debug)]
struct FixtureRunner {
    state: Arc<FixtureState>,
    result: SubtitleProcessOutput,
    output_bytes: Vec<u8>,
}

impl Default for FixtureRunner {
    fn default() -> Self {
        Self {
            state: Arc::new(FixtureState::default()),
            result: SubtitleProcessOutput::default(),
            output_bytes: b"1\n00:00:00,000 --> 00:00:01,000\nconverted\n\n".to_vec(),
        }
    }
}

impl FixtureRunner {
    fn failing(exit_code: i32, standard_error: &str) -> Self {
        Self {
            state: Arc::new(FixtureState::default()),
            result: SubtitleProcessOutput {
                exit_code,
                standard_error: standard_error.to_owned(),
            },
            output_bytes: Vec::new(),
        }
    }

    fn requests(&self) -> Vec<SubtitleProcessRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl SubtitleProcessRunner for FixtureRunner {
    fn run(&self, request: &SubtitleProcessRequest) -> Result<SubtitleProcessOutput, String> {
        self.state.requests.lock().unwrap().push(request.clone());
        if self.result.exit_code == 0 {
            for output_path in &request.output_paths {
                fs::write(output_path, &self.output_bytes).map_err(|error| error.to_string())?;
            }
        }
        Ok(self.result.clone())
    }
}

fn source(cache_directory: &Path, protocol: SubtitleProtocol) -> SubtitleMediaSource {
    SubtitleMediaSource {
        id: "01234567-89ab-cdef-0123-456789abcdef".to_owned(),
        path: PathBuf::from("/media/video.mkv"),
        protocol,
        cache_directory: cache_directory.to_path_buf(),
    }
}

fn external_stream(path: impl Into<PathBuf>) -> SubtitleMediaStream {
    SubtitleMediaStream {
        index: 2,
        path: path.into(),
        codec: String::new(),
        is_external: true,
    }
}

fn build_greek_srt() -> String {
    let mut output = String::new();
    for index in 1..=8 {
        output.push_str(&index.to_string());
        output.push('\n');
        let _ = writeln!(
            output,
            "00:00:{index:02},000 --> 00:00:{:02},000",
            index + 1
        );
        output.push_str(GREEK_TEXT);
        output.push('\n');
        output.push_str("Η γρήγορη καφέ αλεπού πηδάει πάνω από το τεμπέλικο σκυλί.\n\n");
    }
    output
}

fn generate_srt(stream_index: usize, cue_count: usize) -> Vec<u8> {
    let mut output = String::new();
    for index in 0..cue_count {
        let start_seconds = index * 4;
        let end_seconds = start_seconds + 2;
        output.push_str(&(index + 1).to_string());
        output.push('\n');
        let _ = writeln!(
            output,
            "{:02}:{:02}:{:02},000 --> {:02}:{:02}:{:02},000",
            start_seconds / 3_600,
            start_seconds / 60 % 60,
            start_seconds % 60,
            end_seconds / 3_600,
            end_seconds / 60 % 60,
            end_seconds % 60
        );
        let _ = write!(output, "S{stream_index}C{index}\n\n");
    }
    output.into_bytes()
}

fn convert(encoder: &SubtitleEncoder<FixtureRunner>, source: &[u8], stream_index: usize) -> String {
    let output = encoder
        .convert_subtitles(Cursor::new(source), "srt", "vtt", 0, 0, false)
        .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains(&format!("S{stream_index}C{}", CUE_COUNT - 1)));
    text
}

fn convert_all_sequential(
    encoder: &SubtitleEncoder<FixtureRunner>,
    sources: &[Vec<u8>],
) -> Vec<String> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| convert(encoder, source, index))
        .collect()
}

// Official GetReadableFile_Valid_Success: four MemberData rows.
#[test]
fn get_readable_file_valid_success() {
    let temp = TempDirectory::new();
    for (media_protocol, path, expected_format) in [
        (SubtitleProtocol::File, "/media/sub.ass", "ass"),
        (SubtitleProtocol::File, "/media/sub.ssa", "ssa"),
        (SubtitleProtocol::File, "/media/sub.srt", "srt"),
        (SubtitleProtocol::Http, "/media/sub.ass", "ass"),
    ] {
        let runner = FixtureRunner::default();
        let encoder = SubtitleEncoder::new("ffmpeg", runner.clone());
        let result = encoder
            .get_readable_file(&source(temp.path(), media_protocol), &external_stream(path))
            .unwrap();
        assert_eq!(result.path, Path::new(path));
        assert_eq!(result.protocol, SubtitleProtocol::File);
        assert_eq!(result.format, expected_format);
        assert!(result.is_external);
        assert!(runner.requests().is_empty());
    }
}

// Official GetSubtitleStream_NonUtf8LocalFile_ConvertedToUtf8: three encoding rows.
#[test]
fn get_subtitle_stream_non_utf8_local_file_converted_to_utf8() {
    let temp = TempDirectory::new();
    let srt = build_greek_srt();
    let (greek_bytes, _, had_errors) = WINDOWS_1253.encode(&srt);
    assert!(!had_errors);
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(srt.encode_utf16().flat_map(u16::to_le_bytes));

    for (name, bytes) in [
        ("windows-1253", greek_bytes.as_ref()),
        ("iso-8859-7", greek_bytes.as_ref()),
        ("utf-16le", utf16.as_slice()),
    ] {
        let path = temp.path().join(format!("{name}.srt"));
        fs::write(&path, bytes).unwrap();
        let info = SubtitleInfo {
            path,
            protocol: SubtitleProtocol::File,
            format: "srt".to_owned(),
            is_external: true,
        };
        let encoder = SubtitleEncoder::new("ffmpeg", FixtureRunner::default());
        let mut stream = encoder.get_subtitle_stream(&info).unwrap();
        assert!(stream.is_memory_backed());
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(text.contains(GREEK_TEXT));
        assert!(!text.contains(char::REPLACEMENT_CHARACTER));
        assert!(!text.contains('?'));
    }
}

// Official ConvertSubtitles_SequentialCalls_AreDeterministic.
#[test]
fn convert_subtitles_sequential_calls_are_deterministic() {
    let encoder = SubtitleEncoder::new("ffmpeg", FixtureRunner::default());
    let sources = (0..STREAM_COUNT)
        .map(|index| generate_srt(index, CUE_COUNT))
        .collect::<Vec<_>>();
    let first = convert_all_sequential(&encoder, &sources);
    let second = convert_all_sequential(&encoder, &sources);
    assert_eq!(first, second);
}

// Official GetSubtitleStream_Utf8LocalFile_PreservesContent.
#[test]
fn get_subtitle_stream_utf8_local_file_preserves_content() {
    let temp = TempDirectory::new();
    let path = temp.path().join("utf8.srt");
    fs::write(&path, build_greek_srt()).unwrap();
    let info = SubtitleInfo {
        path,
        protocol: SubtitleProtocol::File,
        format: "srt".to_owned(),
        is_external: true,
    };
    let encoder = SubtitleEncoder::new("ffmpeg", FixtureRunner::default());
    let mut stream = encoder.get_subtitle_stream(&info).unwrap();
    assert!(stream.is_file_backed());
    let mut text = String::new();
    stream.read_to_string(&mut text).unwrap();
    assert!(text.contains(GREEK_TEXT));
}

// Official ConvertSubtitles_ConcurrentCalls_MatchSequentialBaseline.
#[test]
fn convert_subtitles_concurrent_calls_match_sequential_baseline() {
    let encoder = Arc::new(SubtitleEncoder::new("ffmpeg", FixtureRunner::default()));
    let sources = Arc::new(
        (0..STREAM_COUNT)
            .map(|index| generate_srt(index, CUE_COUNT))
            .collect::<Vec<_>>(),
    );
    let baseline = convert_all_sequential(&encoder, &sources);

    for _ in 0..10 {
        let handles = (0..STREAM_COUNT)
            .map(|index| {
                let encoder = Arc::clone(&encoder);
                let sources = Arc::clone(&sources);
                thread::spawn(move || convert(&encoder, &sources[index], index))
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results, baseline);
    }
}

#[test]
fn unsupported_external_text_uses_captured_ffmpeg_conversion_request() {
    let temp = TempDirectory::new();
    let input = temp.path().join("subtitle.smi");
    let (bytes, _, had_errors) = WINDOWS_1253.encode(GREEK_TEXT);
    assert!(!had_errors);
    fs::write(&input, bytes).unwrap();
    let runner = FixtureRunner::default();
    let encoder = SubtitleEncoder::new("/opt/jellyfin-ffmpeg/ffmpeg", runner.clone());

    let result = encoder
        .get_readable_file(
            &source(temp.path(), SubtitleProtocol::File),
            &external_stream(&input),
        )
        .unwrap();

    assert_eq!(result.format, "srt");
    assert!(result.path.exists());
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].program,
        Path::new("/opt/jellyfin-ffmpeg/ffmpeg")
    );
    assert_eq!(
        requests[0].arguments,
        [
            "-nostdin",
            "-y",
            "-sub_charenc",
            "windows-1253",
            "-i",
            input.to_str().unwrap(),
            "-c:s",
            "srt",
            result.path.to_str().unwrap(),
        ]
    );
}

#[test]
fn embedded_subtitle_uses_captured_copy_extraction_request() {
    let temp = TempDirectory::new();
    let mut media_source = source(temp.path(), SubtitleProtocol::File);
    media_source.path = temp.path().join("video.mkv");
    fs::write(&media_source.path, b"fixture").unwrap();
    let stream = SubtitleMediaStream {
        index: 7,
        path: PathBuf::new(),
        codec: "ass".to_owned(),
        is_external: false,
    };
    let runner = FixtureRunner::default();
    let encoder = SubtitleEncoder::new("ffmpeg", runner.clone());

    let result = encoder.get_readable_file(&media_source, &stream).unwrap();

    assert_eq!(result.format, "ass");
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].arguments,
        [
            "-nostdin",
            "-y",
            "-i",
            media_source.path.to_str().unwrap(),
            "-copyts",
            "-map",
            "0:7",
            "-an",
            "-vn",
            "-c:s",
            "copy",
            result.path.to_str().unwrap(),
        ]
    );
}

#[test]
fn failed_ffmpeg_conversion_reports_stderr_and_removes_output() {
    let temp = TempDirectory::new();
    let input = temp.path().join("subtitle.smi");
    fs::write(&input, b"plain subtitle").unwrap();
    let encoder = SubtitleEncoder::new("ffmpeg", FixtureRunner::failing(1, "invalid data"));

    let error = encoder
        .get_readable_file(
            &source(temp.path(), SubtitleProtocol::File),
            &external_stream(input),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        SubtitleEncoderError::ProcessFailed {
            exit_code: 1,
            ref standard_error,
            ..
        } if standard_error == "invalid data"
    ));
    assert!(temp.path().read_dir().unwrap().all(|entry| {
        let path = entry.unwrap().path();
        path.extension().is_none_or(|extension| extension != "srt")
    }));
}

#[test]
fn equivalent_formats_return_the_original_file_backed_stream() {
    let temp = TempDirectory::new();
    let encoder = SubtitleEncoder::new("ffmpeg", FixtureRunner::default());
    for (name, input_format, output_format, content) in [
        (
            "same.srt",
            "srt",
            "srt",
            "1\n00:00:01,000 --> 00:00:02,000\nsame\n\n",
        ),
        (
            "superset.ssa",
            "ssa",
            "ass",
            "[Events]\nFormat: Start, End, Text\nDialogue: 0:00:01.00,0:00:02.00,styled\n",
        ),
    ] {
        let path = temp.path().join(name);
        fs::write(&path, content).unwrap();
        let info = SubtitleInfo {
            path,
            protocol: SubtitleProtocol::File,
            format: input_format.to_owned(),
            is_external: true,
        };
        let mut stream = encoder
            .get_subtitles(&info, output_format, 0, 0, false)
            .unwrap();
        assert!(stream.is_file_backed());
        let mut actual = String::new();
        stream.read_to_string(&mut actual).unwrap();
        assert_eq!(actual, content);
    }
}

#[test]
fn in_memory_conversion_filters_window_and_rebases_timestamps() {
    let input = b"1\n00:00:01,000 --> 00:00:02,000\nbefore\n\n\
2\n00:00:04,000 --> 00:00:06,000\ninside\n\n\
3\n00:00:09,000 --> 00:00:10,000\nafter\n\n";
    let encoder = SubtitleEncoder::new("ffmpeg", FixtureRunner::default());

    let output = encoder
        .convert_subtitles(
            Cursor::new(input),
            "srt",
            "vtt",
            30_000_000,
            80_000_000,
            false,
        )
        .unwrap();
    let output = String::from_utf8(output).unwrap();

    assert!(output.starts_with("WEBVTT\n\n"));
    assert!(output.contains("00:00:01.000 --> 00:00:03.000\ninside"));
    assert!(!output.contains("before"));
    assert!(!output.contains("after"));
}

#[test]
fn utf16_sami_conversion_leaves_character_set_to_ffmpeg() {
    let temp = TempDirectory::new();
    let input = temp.path().join("subtitle.sami");
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(
        "<SAMI>subtitle</SAMI>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    fs::write(&input, utf16).unwrap();
    let runner = FixtureRunner::default();
    let encoder = SubtitleEncoder::new("ffmpeg", runner.clone());

    encoder
        .get_readable_file(
            &source(temp.path(), SubtitleProtocol::File),
            &external_stream(input),
        )
        .unwrap();

    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        !requests[0]
            .arguments
            .iter()
            .any(|argument| argument == "-sub_charenc")
    );
}
