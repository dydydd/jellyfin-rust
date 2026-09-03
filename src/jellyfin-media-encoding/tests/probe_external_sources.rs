use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use jellyfin_media_encoding::probing::{
    ExternalMediaSource, ExternalProbeError, ExternalProbeOptions, ExternalSourceProber,
    MediaAttachment, MediaProtocol, ProbeProcessOutput, ProbeProcessRequest, ProbeProcessRunner,
    external_probe_extra_arguments,
};

const PROBE_JSON: &str = include_str!("fixtures/probing/video_webm.json");
const PROBE_WITH_ATTACHMENT_JSON: &str = r#"{
    "streams": [{
        "index": 4,
        "codec_name": "ttf",
        "codec_type": "attachment",
        "codec_tag_string": "TTF",
        "tags": {"filename": "font.ttf", "mimetype": "font/ttf", "comment": "Font"}
    }],
    "format": {"format_name": "matroska,webm", "bit_rate": "1000"}
}"#;

#[derive(Debug, Default)]
struct FixtureState {
    requests: Mutex<Vec<ProbeProcessRequest>>,
}

#[derive(Clone, Debug)]
struct FixtureRunner {
    state: Arc<FixtureState>,
    output: ProbeProcessOutput,
}

impl FixtureRunner {
    fn success(json: &str) -> Self {
        Self {
            state: Arc::new(FixtureState::default()),
            output: ProbeProcessOutput {
                exit_code: 0,
                standard_output: json.to_owned(),
                standard_error: String::new(),
            },
        }
    }

    fn failure(exit_code: i32, standard_error: &str) -> Self {
        Self {
            state: Arc::new(FixtureState::default()),
            output: ProbeProcessOutput {
                exit_code,
                standard_output: String::new(),
                standard_error: standard_error.to_owned(),
            },
        }
    }

    fn requests(&self) -> Vec<ProbeProcessRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl ProbeProcessRunner for FixtureRunner {
    fn run(&self, request: &ProbeProcessRequest) -> Result<ProbeProcessOutput, String> {
        self.state.requests.lock().unwrap().push(request.clone());
        Ok(self.output.clone())
    }
}

fn source(path: &str, protocol: MediaProtocol) -> ExternalMediaSource {
    ExternalMediaSource {
        path: path.to_owned(),
        protocol,
        ..ExternalMediaSource::default()
    }
}

fn argument_value<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}

// Official GetExtraArguments_Forwards_UserAgent: one Fact row.
#[test]
fn get_extra_arguments_forwards_user_agent() {
    let user_agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";
    let mut media_source = source("/path/to/stream", MediaProtocol::Http);
    media_source.required_http_headers =
        BTreeMap::from([("User-Agent".to_owned(), user_agent.to_owned())]);

    let arguments =
        external_probe_extra_arguments(&media_source, &ExternalProbeOptions::default()).unwrap();

    assert_eq!(argument_value(&arguments, "-user_agent"), Some(user_agent));
}

#[test]
fn http_probe_preserves_source_connection_and_applies_normalized_metadata() {
    let user_agent = "Jellyfin fixture/1.0";
    let mut media_source = source("https://media.example.test/movie.webm", MediaProtocol::Http);
    media_source.required_http_headers =
        BTreeMap::from([("User-Agent".to_owned(), user_agent.to_owned())]);
    media_source.container = Some("stale".to_owned());
    media_source.bitrate = Some(1);
    let runner = FixtureRunner::success(PROBE_JSON);
    let prober = ExternalSourceProber::new("/opt/jellyfin-ffmpeg/ffprobe", runner.clone());

    let media_info = prober
        .probe_and_apply(&mut media_source, &ExternalProbeOptions::default())
        .unwrap();

    assert_eq!(media_info.path, "https://media.example.test/movie.webm");
    assert_eq!(media_source.path, media_info.path);
    assert_eq!(media_source.protocol, MediaProtocol::Http);
    assert_eq!(
        media_source
            .required_http_headers
            .get("User-Agent")
            .map(String::as_str),
        Some(user_agent)
    );
    assert_eq!(media_source.container, media_info.container);
    assert_eq!(media_source.bitrate, media_info.bitrate);
    assert_eq!(media_source.media_streams, media_info.media_streams);
    assert_eq!(media_source.media_attachments, media_info.media_attachments);
    assert!(!media_source.media_streams.is_empty());

    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.program, Path::new("/opt/jellyfin-ffmpeg/ffprobe"));
    assert_eq!(request.protocol, MediaProtocol::Http);
    assert_eq!(
        argument_value(&request.arguments, "-i"),
        Some("https://media.example.test/movie.webm")
    );
    assert_eq!(
        argument_value(&request.arguments, "-user_agent"),
        Some(user_agent)
    );
    assert!(
        !request
            .arguments
            .iter()
            .any(|argument| argument == "-show_frames")
    );
}

#[test]
fn probe_and_apply_replaces_media_attachments_from_normalized_probe() {
    let mut media_source = source("/media/movie.mkv", MediaProtocol::File);
    media_source.media_attachments = vec![MediaAttachment {
        index: 99,
        codec: "stale".to_owned(),
        codec_tag: None,
        file_name: Some("stale.bin".to_owned()),
        mime_type: None,
        comment: None,
    }];
    let runner = FixtureRunner::success(PROBE_WITH_ATTACHMENT_JSON);
    let prober = ExternalSourceProber::new("ffprobe", runner);

    let media_info = prober
        .probe_and_apply(&mut media_source, &ExternalProbeOptions::default())
        .unwrap();

    assert_eq!(media_source.media_attachments, media_info.media_attachments);
    assert_eq!(media_source.media_attachments.len(), 1);
    let attachment = &media_source.media_attachments[0];
    assert_eq!(attachment.index, 4);
    assert_eq!(attachment.codec, "ttf");
    assert_eq!(attachment.codec_tag.as_deref(), Some("TTF"));
    assert_eq!(attachment.file_name.as_deref(), Some("font.ttf"));
    assert_eq!(attachment.mime_type.as_deref(), Some("font/ttf"));
    assert_eq!(attachment.comment.as_deref(), Some("Font"));
}

#[test]
fn local_video_probe_uses_file_input_chapters_and_first_frame_metadata() {
    let media_source = source("/media/My Movie.webm", MediaProtocol::File);
    let options = ExternalProbeOptions {
        extract_chapters: true,
        threads: 3,
        configured_analyze_duration: Some("7000000".to_owned()),
        configured_probe_size: Some("50M".to_owned()),
        supports_first_video_frame: true,
        ..ExternalProbeOptions::default()
    };
    let runner = FixtureRunner::success(PROBE_JSON);
    let prober = ExternalSourceProber::new("ffprobe", runner.clone());

    prober.probe(&media_source, &options).unwrap();

    let requests = runner.requests();
    let arguments = &requests[0].arguments;
    assert_eq!(
        argument_value(arguments, "-analyzeduration"),
        Some("7000000")
    );
    assert_eq!(argument_value(arguments, "-probesize"), Some("50M"));
    assert_eq!(
        argument_value(arguments, "-i"),
        Some("file:/media/My Movie.webm")
    );
    assert_eq!(argument_value(arguments, "-threads"), Some("3"));
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "-show_chapters")
    );
    assert!(arguments.iter().any(|argument| argument == "-show_frames"));
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "-only_first_vframe")
    );
}

#[test]
fn source_analyze_duration_and_rtsp_transport_override_configured_defaults() {
    let mut media_source = source("rtsp://camera.example.test/live", MediaProtocol::Rtsp);
    media_source.analyze_duration_ms = Some(3_000);
    let options = ExternalProbeOptions {
        configured_analyze_duration: Some("9000000".to_owned()),
        ..ExternalProbeOptions::default()
    };

    let arguments = external_probe_extra_arguments(&media_source, &options).unwrap();

    assert_eq!(
        argument_value(&arguments, "-analyzeduration"),
        Some("3000000")
    );
    assert_eq!(
        argument_value(&arguments, "-rtsp_transport"),
        Some("tcp+udp")
    );
    assert_eq!(
        argument_value(&arguments, "-rtsp_flags"),
        Some("prefer_tcp")
    );
}

#[test]
fn failed_probe_reports_stderr_and_leaves_source_metadata_unchanged() {
    let mut media_source = source("https://media.example.test/broken", MediaProtocol::Http);
    media_source.container = Some("existing".to_owned());
    media_source.bitrate = Some(42);
    media_source.media_attachments = vec![MediaAttachment {
        index: 1,
        codec: "mjpeg".to_owned(),
        codec_tag: None,
        file_name: Some("poster.jpg".to_owned()),
        mime_type: Some("image/jpeg".to_owned()),
        comment: None,
    }];
    let original = media_source.clone();
    let runner = FixtureRunner::failure(2, "connection refused");
    let prober = ExternalSourceProber::new("ffprobe", runner);

    let error = prober
        .probe_and_apply(&mut media_source, &ExternalProbeOptions::default())
        .unwrap_err();

    assert!(matches!(
        error,
        ExternalProbeError::ProcessFailed {
            exit_code: 2,
            ref standard_error,
            ..
        } if standard_error == "connection refused"
    ));
    assert_eq!(media_source, original);
}

#[test]
fn successful_process_with_invalid_json_returns_normalization_error() {
    let media_source = source("https://media.example.test/invalid", MediaProtocol::Http);
    let runner = FixtureRunner::success("not json");
    let prober = ExternalSourceProber::new("ffprobe", runner);

    let error = prober
        .probe(&media_source, &ExternalProbeOptions::default())
        .unwrap_err();

    assert!(matches!(error, ExternalProbeError::Normalize(_)));
}
