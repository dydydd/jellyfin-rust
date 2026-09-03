use jellyfin_controller::media_encoding::{
    EncodingHelper, EncodingJobInfo, FfmpegVersion, TranscodingJobType,
};
use jellyfin_model::{MediaStream, MediaStreamType};

const BOTH_FILTERS: &str = " -bsf:a noise=drop='lt(pts*tb\\,63.063)',aac_adtstoasc";
const NOISE_ONLY: &str = " -bsf:a noise=drop='lt(pts*tb\\,63.063)'";
const ADTS_ONLY: &str = " -bsf:a aac_adtstoasc";
const DEFAULT_SEEK_TICKS: i64 = 630_630_000;

#[derive(Debug, Clone, Copy)]
struct Case {
    job_type: TranscodingJobType,
    output_video_codec: &'static str,
    output_audio_codec: &'static str,
    audio_stream_codec: &'static str,
    input_container: &'static str,
    start_ticks: i64,
    ffmpeg_version: &'static str,
    segment_container: &'static str,
    media_source_container: &'static str,
    expected: &'static str,
}

macro_rules! case {
    ($job:ident, $video:expr, $audio:expr, $stream:expr, $input:expr, $start:expr, $version:expr, $segment:expr, $source:expr, $expected:expr) => {
        Case {
            job_type: TranscodingJobType::$job,
            output_video_codec: $video,
            output_audio_codec: $audio,
            audio_stream_codec: $stream,
            input_container: $input,
            start_ticks: $start,
            ffmpeg_version: $version,
            segment_container: $segment,
            media_source_container: $source,
            expected: $expected,
        }
    };
}

const OFFICIAL_CASES: &[Case] = &[
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "ts",
        BOTH_FILTERS
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "aac",
        BOTH_FILTERS
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "hls",
        BOTH_FILTERS
    ),
    case!(
        Progressive,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "ts",
        ADTS_ONLY
    ),
    case!(
        Hls,
        "copy",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "ts",
        ADTS_ONLY
    ),
    case!(
        Hls,
        "libx264",
        "aac",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "ts",
        ADTS_ONLY
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "wtv",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "ts",
        ADTS_ONLY
    ),
    case!(
        Hls, "libx264", "copy", "aac", "ts", 0, "5.0", "mp4", "ts", ADTS_ONLY
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "4.4.6",
        "mp4",
        "ts",
        ADTS_ONLY
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "ts",
        "ts",
        NOISE_ONLY
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "aac",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "mkv",
        NOISE_ONLY
    ),
    case!(
        Hls,
        "libx264",
        "copy",
        "ac3",
        "ts",
        DEFAULT_SEEK_TICKS,
        "5.0",
        "mp4",
        "ts",
        NOISE_ONLY
    ),
];

#[test]
fn audio_bit_stream_arguments_apply_all_official_gates() {
    assert_eq!(OFFICIAL_CASES.len(), 12);
    for case in OFFICIAL_CASES {
        let state = create_state(
            case.job_type,
            case.output_video_codec,
            case.output_audio_codec,
            case.audio_stream_codec,
            case.input_container,
            case.start_ticks,
        );
        let helper = EncodingHelper::new(case.ffmpeg_version.parse::<FfmpegVersion>().unwrap());
        assert_eq!(
            helper.get_audio_bit_stream_arguments(
                &state,
                case.segment_container,
                case.media_source_container,
            ),
            case.expected,
            "{case:?}"
        );
    }
}

#[test]
fn codec_and_container_checks_are_case_insensitive() {
    let state = create_state(
        TranscodingJobType::Hls,
        "LIBX264",
        "COPY",
        "AAC-LATM",
        "TS",
        DEFAULT_SEEK_TICKS,
    );
    let helper = EncodingHelper::new(FfmpegVersion::new(5, 0));

    assert_eq!(
        helper.get_audio_bit_stream_arguments(&state, ".MP4", "HLS"),
        BOTH_FILTERS
    );
}

fn create_state(
    job_type: TranscodingJobType,
    output_video_codec: &str,
    output_audio_codec: &str,
    audio_stream_codec: &str,
    input_container: &str,
    start_time_ticks: i64,
) -> EncodingJobInfo {
    EncodingJobInfo {
        transcoding_type: job_type,
        is_video_request: true,
        output_video_codec: output_video_codec.to_owned(),
        output_audio_codec: output_audio_codec.to_owned(),
        input_container: input_container.to_owned(),
        audio_stream: Some(MediaStream {
            stream_type: MediaStreamType::Audio,
            codec: Some(audio_stream_codec.to_owned()),
            ..MediaStream::default()
        }),
        start_time_ticks: Some(start_time_ticks),
        ..EncodingJobInfo::default()
    }
}
