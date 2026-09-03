use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use jellyfin_controller::media_encoding::{
    EncodingHelper, EncodingJobInfo, FfmpegVersion, TranscodingJobType,
};
use jellyfin_model::{MediaSourceInfo, MediaStream, MediaStreamType, SubtitleDeliveryMethod};

fn helper() -> EncodingHelper {
    EncodingHelper::new(FfmpegVersion::new(5, 0))
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = env::temp_dir().join(format!(
            "jellyfin-encoding-helper-{}-{}",
            process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove temporary directory");
    }
}

fn stream(index: i32, stream_type: MediaStreamType, codec: &str) -> MediaStream {
    MediaStream {
        index,
        stream_type,
        codec: Some(codec.to_owned()),
        ..MediaStream::default()
    }
}

fn external_subtitle(index: i32, codec: &str, path: &str) -> MediaStream {
    MediaStream {
        is_external: true,
        supports_external_stream: true,
        path: Some(path.to_owned()),
        ..stream(index, MediaStreamType::Subtitle, codec)
    }
}

fn video_state(
    subtitle: Option<MediaStream>,
    delivery_method: SubtitleDeliveryMethod,
    additional_streams: Option<Vec<MediaStream>>,
) -> EncodingJobInfo {
    let video = stream(0, MediaStreamType::Video, "h264");
    let audio = stream(1, MediaStreamType::Audio, "aac");
    let mut media_streams = vec![video.clone(), audio.clone()];
    match additional_streams {
        Some(streams) => media_streams.extend(streams),
        None => media_streams.extend(subtitle.iter().cloned()),
    }

    EncodingJobInfo {
        transcoding_type: TranscodingJobType::Progressive,
        is_video_request: true,
        is_input_video: true,
        input_container: "mkv".to_owned(),
        media_source: MediaSourceInfo {
            container: Some("mkv".to_owned()),
            media_streams,
            ..MediaSourceInfo::default()
        },
        video_stream: Some(video),
        audio_stream: Some(audio),
        subtitle_stream: subtitle,
        subtitle_delivery_method: delivery_method,
        ..EncodingJobInfo::default()
    }
}

#[test]
fn get_map_args_no_subtitle_excludes_all_subtitles() {
    let state = video_state(None, SubtitleDeliveryMethod::Drop, None);
    let args = helper().get_map_args(&state);

    assert!(args.contains("-map -0:s"));
    assert!(!args.contains("-map 1:"));
}

#[test]
fn get_map_args_internal_srt_maps_from_primary_input() {
    let subtitle = stream(2, MediaStreamType::Subtitle, "srt");
    let state = video_state(Some(subtitle), SubtitleDeliveryMethod::Embed, None);
    let args = helper().get_map_args(&state);

    assert!(args.contains("-map 0:2"));
    assert!(!args.contains("-map 1:"));
}

#[test]
fn get_map_args_internal_subtitle_at_higher_index_maps_correct_index() {
    let subtitle0 = stream(2, MediaStreamType::Subtitle, "srt");
    let subtitle1 = stream(3, MediaStreamType::Subtitle, "ass");
    let state = video_state(
        Some(subtitle1.clone()),
        SubtitleDeliveryMethod::Embed,
        Some(vec![subtitle0, subtitle1]),
    );

    assert!(helper().get_map_args(&state).contains("-map 0:3"));
}

#[test]
fn get_map_args_external_srt_maps_first_stream_from_input_one() {
    let subtitle = external_subtitle(2, "srt", "/media/movie.en.srt");
    let state = video_state(Some(subtitle), SubtitleDeliveryMethod::Embed, None);

    assert!(helper().get_map_args(&state).contains("-map 1:0"));
}

#[test]
fn get_map_args_second_external_srt_still_maps_input_one_stream_zero() {
    let subtitle0 = external_subtitle(2, "srt", "/media/movie.en.srt");
    let subtitle1 = external_subtitle(3, "srt", "/media/movie.fr.srt");
    let state = video_state(
        Some(subtitle1.clone()),
        SubtitleDeliveryMethod::Embed,
        Some(vec![subtitle0, subtitle1]),
    );

    assert!(helper().get_map_args(&state).contains("-map 1:0"));
}

#[test]
fn get_map_args_mks_first_track_maps_in_file_index_zero() {
    let subtitle0 = external_subtitle(2, "subrip", "/media/movie.mks");
    let subtitle1 = external_subtitle(3, "ass", "/media/movie.mks");
    let state = video_state(
        Some(subtitle0.clone()),
        SubtitleDeliveryMethod::Embed,
        Some(vec![subtitle0, subtitle1]),
    );

    assert!(helper().get_map_args(&state).contains("-map 1:0"));
}

#[test]
fn get_map_args_mks_second_track_maps_in_file_index_one() {
    let subtitle0 = external_subtitle(2, "subrip", "/media/movie.mks");
    let subtitle1 = external_subtitle(3, "ass", "/media/movie.mks");
    let subtitle2 = external_subtitle(4, "subrip", "/media/movie.mks");
    let state = video_state(
        Some(subtitle1.clone()),
        SubtitleDeliveryMethod::Embed,
        Some(vec![subtitle0, subtitle1, subtitle2]),
    );

    assert!(helper().get_map_args(&state).contains("-map 1:1"));
}

fn assert_vobsub_path(
    delivery_method: SubtitleDeliveryMethod,
    create_idx_file: bool,
    expected_filename: &str,
) {
    let temporary = TestDirectory::new();
    let sub_path = temporary.path().join("movie.sub");
    fs::write(&sub_path, "dummy").expect("write VobSub data");
    if create_idx_file {
        fs::write(temporary.path().join("movie.idx"), "dummy").expect("write VobSub index");
    }

    let subtitle = external_subtitle(2, "dvdsub", &sub_path.to_string_lossy());
    let mut state = video_state(Some(subtitle), delivery_method, None);
    state.media_path = Some("/media/movie.mkv".to_owned());
    let arguments = helper().get_input_argument(&state);

    assert!(arguments.contains(expected_filename), "{arguments}");
}

macro_rules! vobsub_path_test {
    ($name:ident, $delivery:expr, $create_idx:literal, $expected:literal) => {
        #[test]
        fn $name() {
            assert_vobsub_path($delivery, $create_idx, $expected);
        }
    };
}

vobsub_path_test!(
    get_input_argument_embed_uses_idx_when_present,
    SubtitleDeliveryMethod::Embed,
    true,
    "movie.idx"
);
vobsub_path_test!(
    get_input_argument_encode_uses_idx_when_present,
    SubtitleDeliveryMethod::Encode,
    true,
    "movie.idx"
);
vobsub_path_test!(
    get_input_argument_embed_uses_sub_without_idx,
    SubtitleDeliveryMethod::Embed,
    false,
    "movie.sub"
);
vobsub_path_test!(
    get_input_argument_encode_uses_sub_without_idx,
    SubtitleDeliveryMethod::Encode,
    false,
    "movie.sub"
);

fn assert_audio_sample_rate(codec: &str, requested: i32, expected: i32) {
    let audio = MediaStream {
        sample_rate: Some(96_000),
        ..stream(0, MediaStreamType::Audio, "flac")
    };
    let state = EncodingJobInfo {
        transcoding_type: TranscodingJobType::Progressive,
        output_audio_codec: codec.to_owned(),
        output_audio_sample_rate: Some(requested),
        input_container: "flac".to_owned(),
        media_path: Some("/media/track.flac".to_owned()),
        media_source: MediaSourceInfo {
            container: Some("flac".to_owned()),
            media_streams: vec![audio.clone()],
            ..MediaSourceInfo::default()
        },
        audio_stream: Some(audio),
        ..EncodingJobInfo::default()
    };

    let arguments = helper().get_progressive_audio_full_command_line(&state, "/tmp/out");
    assert!(
        arguments.contains(&format!("-ar {expected}")),
        "{arguments}"
    );
}

macro_rules! audio_sample_rate_test {
    ($name:ident, $codec:literal, $requested:literal, $expected:literal) => {
        #[test]
        fn $name() {
            assert_audio_sample_rate($codec, $requested, $expected);
        }
    };
}

audio_sample_rate_test!(
    audio_sample_rate_aac_44100_is_preserved,
    "aac",
    44_100,
    44_100
);
audio_sample_rate_test!(
    audio_sample_rate_aac_48000_is_preserved,
    "aac",
    48_000,
    48_000
);
audio_sample_rate_test!(
    audio_sample_rate_mp3_22050_is_preserved,
    "mp3",
    22_050,
    22_050
);
audio_sample_rate_test!(
    audio_sample_rate_flac_96000_is_preserved,
    "flac",
    96_000,
    96_000
);
audio_sample_rate_test!(
    audio_sample_rate_opus_44100_clamps_up,
    "opus",
    44_100,
    48_000
);
audio_sample_rate_test!(
    audio_sample_rate_opus_22050_clamps_up,
    "opus",
    22_050,
    24_000
);
audio_sample_rate_test!(
    audio_sample_rate_opus_8000_is_preserved,
    "opus",
    8_000,
    8_000
);
