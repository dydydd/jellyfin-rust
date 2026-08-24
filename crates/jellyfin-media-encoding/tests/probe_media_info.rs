use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use jellyfin_media_encoding::probing::{
    AudioSpatialFormat, MediaInfo, MediaPersonKind, MediaStreamType, ProbeContext,
    normalize_probe_file, normalize_probe_json,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probing")
        .join(name)
}

fn normalize(name: &str, path: &str, is_audio: bool) -> MediaInfo {
    normalize_probe_file(fixture(name), ProbeContext { path, is_audio })
        .expect("official probe fixture should normalize")
}

#[test]
fn media_metadata_success() {
    let result = normalize("video_metadata.json", "video_metadata.mkv", false);
    assert_eq!(result.container.as_deref(), Some("mkv"));
    assert_eq!(result.runtime_ticks, Some(10_000_000));
    assert_eq!(result.media_streams.len(), 3);
    let video = result.video_stream().expect("video stream");
    assert_eq!(video.aspect_ratio.as_deref(), Some("4:3"));
    assert_eq!(video.average_frame_rate, Some(25.0));
    assert_eq!(video.bit_depth, Some(8));
    assert_eq!(video.bit_rate, None);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.codec_time_base.as_deref(), Some("1/50"));
    assert_eq!(video.height, Some(240));
    assert_eq!(video.width, Some(320));
    assert_eq!(video.index, 0);
    assert!(!video.is_anamorphic());
    assert!(video.is_avc());
    assert!(video.is_default());
    assert!(!video.is_external());
    assert!(!video.is_forced());
    assert!(!video.is_hearing_impaired());
    assert!(!video.is_interlaced());
    assert!(!video.is_text_subtitle_stream());
    assert_eq!(video.level, Some(13.0));
    assert_eq!(video.nal_length_size.as_deref(), Some("4"));
    assert_eq!(video.pixel_format.as_deref(), Some("yuv444p"));
    assert_eq!(video.profile.as_deref(), Some("High 4:4:4 Predictive"));
    assert_eq!(video.real_frame_rate, Some(25.0));
    assert_eq!(video.ref_frames, Some(1));
    assert_eq!(video.time_base.as_deref(), Some("1/1000"));
    assert_eq!(video.stream_type, MediaStreamType::Video);
    assert_eq!(video.dv_version_major, Some(1));
    assert_eq!(video.dv_version_minor, Some(0));
    assert_eq!(video.dv_profile, Some(5));
    assert_eq!(video.dv_level, Some(6));
    assert_eq!(video.rpu_present_flag, Some(1));
    assert_eq!(video.el_present_flag, Some(0));
    assert_eq!(video.bl_present_flag, Some(1));
    assert_eq!(video.dv_bl_signal_compatibility_id, Some(0));
    assert_eq!(video.rotation, Some(-180));

    let audio1 = &result.media_streams[1];
    assert_eq!(audio1.codec, "eac3");
    assert!(audio1.is_original());
    assert_eq!(
        audio1.audio_spatial_format,
        Some(AudioSpatialFormat::DolbyAtmos)
    );
    let audio2 = &result.media_streams[2];
    assert_eq!(audio2.codec, "dts");
    assert!(!audio2.is_original());
    assert_eq!(audio2.audio_spatial_format, Some(AudioSpatialFormat::DtsX));
    assert!(result.chapters.is_empty());
    assert_eq!(result.overview.as_deref(), Some("Just color bars"));
}

#[test]
fn mp4_metadata_success() {
    let result = normalize("video_mp4_metadata.json", "video_mp4_metadata.mkv", false);
    assert_eq!(result.media_streams.len(), 6);
    let video = result.video_stream().expect("video stream");
    assert_eq!(video.index, 0);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.profile.as_deref(), Some("High"));
    assert_eq!(video.stream_type, MediaStreamType::Video);
    assert_eq!(video.height, Some(358));
    assert_eq!(video.width, Some(720));
    assert_eq!(video.aspect_ratio.as_deref(), Some("2.40:1"));
    assert!(video.is_anamorphic());
    assert_eq!(video.pixel_format.as_deref(), Some("yuv420p"));
    assert_eq!(video.level, Some(31.0));
    assert_eq!(video.ref_frames, Some(1));
    assert!(video.is_avc());
    assert_eq!(video.real_frame_rate, Some(120.0));
    assert_eq!(video.time_base.as_deref(), Some("1/90000"));
    assert_eq!(video.color_range.as_deref(), Some("tv"));
    assert_eq!(video.color_space.as_deref(), Some("smpte170m"));
    assert_eq!(video.color_transfer.as_deref(), Some("bt709"));
    assert_eq!(video.color_primaries.as_deref(), Some("smpte170m"));
    assert_eq!(video.bit_rate, Some(1_147_365));
    assert_eq!(video.bit_depth, Some(8));
    assert!(video.is_default());
    assert_eq!(video.language.as_deref(), Some("und"));

    let main_audio = &result.media_streams[1];
    assert_eq!(main_audio.stream_type, MediaStreamType::Audio);
    assert_eq!(main_audio.codec, "aac");
    assert_eq!(main_audio.channels, Some(7));
    assert!(main_audio.is_default());
    assert!(!main_audio.is_original());
    assert_eq!(main_audio.language.as_deref(), Some("eng"));
    assert_eq!(main_audio.title.as_deref(), Some("Surround 6.1"));

    let commentary_audio = &result.media_streams[2];
    assert_eq!(commentary_audio.stream_type, MediaStreamType::Audio);
    assert_eq!(commentary_audio.codec, "aac");
    assert_eq!(commentary_audio.channels, Some(2));
    assert!(!commentary_audio.is_default());
    assert_eq!(commentary_audio.language.as_deref(), Some("eng"));
    assert_eq!(commentary_audio.title.as_deref(), Some("Commentary"));

    let spanish = &result.media_streams[3];
    assert_eq!(spanish.language.as_deref(), Some("spa"));
    assert_eq!(spanish.stream_type, MediaStreamType::Subtitle);
    assert_eq!(spanish.codec, "DVDSUB");
    assert_eq!(spanish.title, None);
    assert!(!spanish.is_hearing_impaired());

    let english = &result.media_streams[4];
    assert_eq!(english.language.as_deref(), Some("eng"));
    assert_eq!(english.stream_type, MediaStreamType::Subtitle);
    assert_eq!(english.codec, "mov_text");
    assert_eq!(english.title, None);
    assert!(english.is_hearing_impaired());

    let commentary = &result.media_streams[5];
    assert_eq!(commentary.language.as_deref(), Some("eng"));
    assert_eq!(commentary.stream_type, MediaStreamType::Subtitle);
    assert_eq!(commentary.codec, "mov_text");
    assert_eq!(commentary.title.as_deref(), Some("Commentary"));
    assert!(!commentary.is_hearing_impaired());
}

#[test]
fn transport_stream_success() {
    let result = normalize("video_ts.json", "video_metadata.mkv", false);
    assert_eq!(result.media_streams.len(), 2);
    assert!(!result.media_streams[0].is_avc());
}

#[test]
fn webm_success() {
    let result = normalize("video_webm.json", "video_metadata.webm", false);
    assert_eq!(result.container.as_deref(), Some("mkv,webm"));
    assert_eq!(result.runtime_ticks, Some(1_177_010_000));
    assert_eq!(result.media_streams.len(), 2);
    assert_eq!(result.media_streams[0].width, Some(540));
    assert_eq!(result.media_streams[0].height, Some(360));
}

#[test]
fn webm_like_mkv_with_subtitle_is_mkv() {
    let result = normalize(
        "video_web_like_mkv_with_subtitle.json",
        "video_metadata.mkv",
        false,
    );
    assert_eq!(result.container.as_deref(), Some("mkv"));
    assert_eq!(result.media_streams.len(), 3);
}

#[test]
fn progressive_video_without_field_order_success() {
    let result = normalize(
        "video_progressive_no_field_order.json",
        "video_progressive_no_field_order.mp4",
        false,
    );
    assert_eq!(result.media_streams.len(), 2);
    assert_video(
        &result,
        ExpectedVideo {
            profile: "Main",
            width: 1920,
            height: 1080,
            interlaced: false,
            level: 41.0,
            frame_rate: 23.976_025,
            time_base: "1/24000",
            bit_rate: 3_948_341,
        },
    );
}

#[test]
fn second_progressive_video_without_field_order_success() {
    let result = normalize(
        "video_progressive_no_field_order2.json",
        "video_progressive_no_field_order2.mp4",
        false,
    );
    assert_eq!(result.media_streams.len(), 1);
    assert_video(
        &result,
        ExpectedVideo {
            profile: "High",
            width: 1280,
            height: 720,
            interlaced: false,
            level: 31.0,
            frame_rate: 25.0,
            time_base: "1/12800",
            bit_rate: 53_288,
        },
    );
}

#[test]
fn interlaced_video_success() {
    let result = normalize("video_interlaced.json", "video_interlaced.mp4", false);
    assert_eq!(result.media_streams.len(), 1);
    assert_video(
        &result,
        ExpectedVideo {
            profile: "High",
            width: 1280,
            height: 720,
            interlaced: true,
            level: 40.0,
            frame_rate: 25.0,
            time_base: "1/12800",
            bit_rate: 56_945,
        },
    );
}

#[derive(Clone, Copy)]
struct ExpectedVideo<'a> {
    profile: &'a str,
    width: i32,
    height: i32,
    interlaced: bool,
    level: f64,
    frame_rate: f32,
    time_base: &'a str,
    bit_rate: i64,
}

fn assert_video(result: &MediaInfo, expected: ExpectedVideo<'_>) {
    let video = result.video_stream().expect("video stream");
    assert_eq!(video.index, 0);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.profile.as_deref(), Some(expected.profile));
    assert_eq!(video.stream_type, MediaStreamType::Video);
    assert_eq!(video.height, Some(expected.height));
    assert_eq!(video.width, Some(expected.width));
    assert_eq!(video.is_interlaced(), expected.interlaced);
    assert_eq!(video.aspect_ratio.as_deref(), Some("16:9"));
    assert_eq!(video.pixel_format.as_deref(), Some("yuv420p"));
    assert_eq!(video.level, Some(expected.level));
    assert_eq!(video.ref_frames, Some(1));
    assert!(video.is_avc());
    assert_eq!(video.real_frame_rate, Some(expected.frame_rate));
    assert_eq!(video.time_base.as_deref(), Some(expected.time_base));
    assert_eq!(video.bit_rate, Some(expected.bit_rate));
    assert_eq!(video.bit_depth, Some(8));
    assert!(video.is_default());
}

#[test]
fn hdr10_plus_flag_is_read_from_the_first_video_frame() {
    let probe = r#"
    {
      "streams": [
        {
          "index": 0,
          "codec_type": "video",
          "codec_name": "hevc",
          "color_range": "pc",
          "color_space": "bt2020nc",
          "color_transfer": "smpte2084",
          "color_primaries": "bt2020"
        }
      ],
      "frames": [
        {
          "stream_index": 0,
          "side_data_list": [
            {
              "side_data_type": "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
            }
          ]
        }
      ]
    }
    "#;
    let result = normalize_probe_json(
        probe,
        ProbeContext {
            path: "hdr10plus.mkv",
            is_audio: false,
        },
    )
    .expect("HDR10+ probe should normalize");
    let video = result.video_stream().expect("video stream");
    assert_eq!(video.color_range.as_deref(), Some("pc"));
    assert_eq!(video.color_space.as_deref(), Some("bt2020nc"));
    assert_eq!(video.color_transfer.as_deref(), Some("smpte2084"));
    assert_eq!(video.color_primaries.as_deref(), Some("bt2020"));
    assert_eq!(video.hdr10_plus_present_flag, Some(true));
}

#[test]
fn missing_video_bitrate_is_estimated_from_container() {
    let result = normalize(
        "video_missing_video_bitrate.json",
        "video_missing_video_bitrate.mp4",
        false,
    );
    assert_eq!(result.media_streams.len(), 2);
    let audio = result
        .media_streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Audio)
        .expect("audio stream");
    assert_eq!(audio.bit_rate, Some(128_000));
    assert_eq!(
        result.video_stream().and_then(|stream| stream.bit_rate),
        Some(5_000_000)
    );
    assert_eq!(result.bitrate, Some(5_128_000));
}

#[test]
fn nanosecond_duration_tag_computes_bitrate_from_bytes() {
    let result = normalize(
        "video_nanosecond_duration_bitrate.json",
        "video_nanosecond_duration_bitrate.mkv",
        false,
    );
    assert_eq!(
        result.video_stream().and_then(|stream| stream.bit_rate),
        Some(800_000)
    );
}

#[test]
fn unknown_audio_bitrate_prevents_video_estimate() {
    let result = normalize(
        "video_missing_video_bitrate_unknown_audio.json",
        "video_missing_video_bitrate_unknown_audio.mp4",
        false,
    );
    assert_eq!(result.media_streams.len(), 2);
    assert_eq!(
        result.video_stream().and_then(|stream| stream.bit_rate),
        None
    );
    let audio = result
        .media_streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Audio)
        .expect("audio stream");
    assert_eq!(audio.bit_rate, None);
    assert_eq!(result.bitrate, Some(5_128_000));
}

#[test]
fn video_with_single_frame_mjpeg_success() {
    let result = normalize(
        "video_single_frame_mjpeg.json",
        "video_interlaced.mp4",
        false,
    );
    assert_eq!(result.media_streams.len(), 3);
    let video = result.video_stream().expect("video stream");
    assert_eq!(video.index, 0);
    assert_eq!(video.codec, "h264");
    assert_eq!(video.profile.as_deref(), Some("High"));
    assert_eq!(video.height, Some(1080));
    assert_eq!(video.width, Some(1920));
    assert!(!video.is_interlaced());
    assert_eq!(video.aspect_ratio.as_deref(), Some("16:9"));
    assert_eq!(video.pixel_format.as_deref(), Some("yuv420p"));
    assert_eq!(video.level, Some(42.0));
    assert_eq!(video.ref_frames, Some(1));
    assert!(video.is_avc());
    assert_eq!(video.real_frame_rate, Some(50.0));
    assert_eq!(video.time_base.as_deref(), Some("1/1000"));
    assert_eq!(video.bit_depth, Some(8));
    assert!(video.is_default());
    assert_eq!(result.media_streams[2].codec, "mjpeg");
}

#[test]
fn music_video_metadata_success() {
    let result = normalize("music_video_metadata.json", "music_video.mkv", false);
    assert_eq!(result.name.as_deref(), Some("The Title"));
    assert_eq!(result.forced_sort_name.as_deref(), Some("Title, The"));
    assert_eq!(result.artists, ["The Artist"]);
    assert_eq!(result.album.as_deref(), Some("Album"));
    assert_eq!(result.production_year, Some(2021));
    assert_eq!(
        result.premiere_date,
        Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).single()
    );
}

#[test]
fn music_year_only_original_date_success() {
    let result = normalize("music_year_only_metadata.json", "music.flac", true);
    assert_eq!(result.name.as_deref(), Some("Baker Street"));
    assert_eq!(result.artists, ["Gerry Rafferty"]);
    assert_eq!(result.album.as_deref(), Some("City to City"));
    assert_eq!(result.production_year, Some(1978));
    assert_eq!(
        result.premiere_date,
        Utc.with_ymd_and_hms(1978, 1, 1, 0, 0, 0).single()
    );
    for genre in ["Electronic", "Ambient", "Pop", "Jazz"] {
        assert!(result.genres.iter().any(|value| value == genre));
    }
    assert_eq!(result.media_attachments.len(), 1);
}

#[test]
fn music_metadata_success() {
    let result = normalize("music_metadata.json", "music.flac", true);
    assert_eq!(result.name.as_deref(), Some("UP NO MORE"));
    assert_eq!(result.artists, ["TWICE"]);
    assert_eq!(result.album.as_deref(), Some("Eyes wide open"));
    assert_eq!(result.production_year, Some(2020));
    assert_eq!(
        result.premiere_date,
        Utc.with_ymd_and_hms(2020, 10, 26, 0, 0, 0).single()
    );
    assert_eq!(result.people.len(), 22);
    assert_person(&result, 0, "Krysta Youngs", MediaPersonKind::Composer, None);
    assert_person(&result, 1, "Julia Ross", MediaPersonKind::Composer, None);
    assert_person(&result, 2, "Yiwoomin", MediaPersonKind::Composer, None);
    assert_person(&result, 3, "Ji-hyo Park", MediaPersonKind::Lyricist, None);
    assert_person(
        &result,
        4,
        "Yiwoomin",
        MediaPersonKind::Actor,
        Some("Electric Piano"),
    );
    assert_eq!(result.genres.len(), 4);
    for genre in ["Electronic", "Trance", "Dance", "Jazz"] {
        assert!(result.genres.iter().any(|value| value == genre));
    }
    assert_eq!(result.media_attachments.len(), 1);
}

fn assert_person(
    result: &MediaInfo,
    index: usize,
    name: &str,
    kind: MediaPersonKind,
    role: Option<&str>,
) {
    assert_eq!(result.people[index].name, name);
    assert_eq!(result.people[index].kind, kind);
    assert_eq!(result.people[index].role.as_deref(), role);
}

#[test]
fn nonempty_attachment_and_chapter_are_normalized() {
    let input = r#"{
        "streams": [{
            "index": 4,
            "codec_name": "ttf",
            "codec_type": "attachment",
            "codec_tag_string": "[0][0][0][0]",
            "tags": {"filename": "font.ttf", "mimetype": "font/ttf", "comment": "Font"}
        }],
        "chapters": [{"start_time": "12.3456", "tags": {"title": "Opening"}}]
    }"#;
    let result = normalize_probe_json(
        input,
        ProbeContext {
            path: "sample.mkv",
            is_audio: false,
        },
    )
    .expect("synthetic probe JSON should normalize");
    assert_eq!(result.media_attachments.len(), 1);
    let attachment = &result.media_attachments[0];
    assert_eq!(attachment.index, 4);
    assert_eq!(attachment.codec, "ttf");
    assert_eq!(attachment.file_name.as_deref(), Some("font.ttf"));
    assert_eq!(attachment.mime_type.as_deref(), Some("font/ttf"));
    assert_eq!(attachment.comment.as_deref(), Some("Font"));
    assert_eq!(result.chapters.len(), 1);
    assert_eq!(result.chapters[0].name.as_deref(), Some("Opening"));
    assert_eq!(result.chapters[0].start_position_ticks, 123_460_000);
}
