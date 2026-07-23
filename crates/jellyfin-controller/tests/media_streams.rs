use jellyfin_controller::{MediaStreamMapper, MediaStreamPathMapper};
use jellyfin_data::{PersistedMediaStream, PersistedMediaStreamType};
use jellyfin_model::{MediaStream, MediaStreamType, VideoRangeType};

#[derive(Clone, Copy)]
struct TestPathMapper;

impl MediaStreamPathMapper for TestPathMapper {
    fn path_to_save(&self, path: &str) -> Option<String> {
        path.strip_prefix("/media/")
            .map(|relative| format!("virtual:{relative}"))
    }

    fn restore_path(&self, path: &str) -> Option<String> {
        path.strip_prefix("virtual:")
            .map(|relative| format!("/media/{relative}"))
    }
}

#[test]
fn persisted_audio_streams_project_localized_official_dto_fields() {
    let mapper = MediaStreamMapper::new(TestPathMapper, "en-US");
    let mut persisted = minimal_persisted(1, PersistedMediaStreamType::Audio);
    persisted.language = Some("ger".to_owned());
    persisted.path = Some("virtual:movie.ger.ac3".to_owned());
    persisted.profile = Some("Dolby Atmos".to_owned());
    persisted.is_default = true;
    persisted.is_external = true;
    persisted.is_original = true;

    let stream = mapper.to_api(persisted);

    assert_eq!(stream.stream_type, MediaStreamType::Audio);
    assert_eq!(stream.index, 1);
    assert_eq!(stream.language.as_deref(), Some("deu"));
    assert_eq!(stream.localized_language.as_deref(), Some("German"));
    assert_eq!(stream.localized_default.as_deref(), Some("Default"));
    assert_eq!(stream.localized_external.as_deref(), Some("External"));
    assert_eq!(stream.localized_original.as_deref(), Some("Original"));
    assert!(stream.localized_forced.is_none());
    assert_eq!(stream.path.as_deref(), Some("/media/movie.ger.ac3"));
    assert!(!stream.is_interlaced);
    assert!(!stream.is_hearing_impaired);
    assert_eq!(
        stream.display_title().as_deref(),
        Some("German - Dolby Atmos - Default - External - Original")
    );
}

#[test]
fn persisted_video_streams_preserve_repository_fields_and_compute_range_type() {
    let mapper = MediaStreamMapper::new(TestPathMapper, "en-US");
    let mut persisted = minimal_persisted(0, PersistedMediaStreamType::Video);
    persisted.codec = Some("hevc".to_owned());
    persisted.codec_tag = Some("hvc1".to_owned());
    persisted.color_primaries = Some("bt2020".to_owned());
    persisted.color_space = Some("bt2020nc".to_owned());
    persisted.color_transfer = Some("smpte2084".to_owned());
    persisted.comment = Some("main feature".to_owned());
    persisted.time_base = Some("1/90000".to_owned());
    persisted.codec_time_base = Some("1/48".to_owned());
    persisted.nal_length_size = Some("4".to_owned());
    persisted.is_avc = Some(false);
    persisted.aspect_ratio = Some("16:9".to_owned());
    persisted.pixel_format = Some("yuv420p10le".to_owned());
    persisted.level = Some(5.1);
    persisted.is_interlaced = Some(true);
    persisted.is_hearing_impaired = Some(true);

    let stream = mapper.to_api(persisted);

    assert_eq!(stream.stream_type, MediaStreamType::Video);
    assert_eq!(stream.video_range_type, VideoRangeType::Hdr10);
    assert_eq!(stream.color_primaries.as_deref(), Some("bt2020"));
    assert_eq!(stream.color_space.as_deref(), Some("bt2020nc"));
    assert_eq!(stream.comment.as_deref(), Some("main feature"));
    assert_eq!(stream.time_base.as_deref(), Some("1/90000"));
    assert_eq!(stream.codec_time_base.as_deref(), Some("1/48"));
    assert_eq!(stream.nal_length_size.as_deref(), Some("4"));
    assert_eq!(stream.is_avc, Some(false));
    assert_eq!(stream.aspect_ratio.as_deref(), Some("16:9"));
    assert_eq!(stream.pixel_format.as_deref(), Some("yuv420p10le"));
    assert!((stream.level.unwrap() - f64::from(5.1_f32)).abs() < 0.0001);
    assert!(stream.is_interlaced);
    assert!(stream.is_hearing_impaired);
    assert!(stream.localized_default.is_none());
}

#[test]
fn api_streams_save_to_persisted_repository_shape() {
    let mapper = MediaStreamMapper::new(TestPathMapper, "en-US");
    let stream = MediaStream {
        index: -1,
        stream_type: MediaStreamType::Subtitle,
        codec: Some("srt".to_owned()),
        language: Some("eng".to_owned()),
        channel_layout: Some("stereo".to_owned()),
        profile: Some("main".to_owned()),
        aspect_ratio: Some("4:3".to_owned()),
        path: Some("/media/movie.eng.srt".to_owned()),
        is_interlaced: false,
        bit_rate: Some(92),
        channels: Some(2),
        sample_rate: Some(48_000),
        is_default: true,
        is_forced: true,
        is_external: true,
        is_original: false,
        height: Some(720),
        width: Some(1280),
        average_frame_rate: Some(23.976),
        real_frame_rate: Some(24.0),
        level: Some(4.1),
        pixel_format: Some("yuv420p".to_owned()),
        bit_depth: Some(8),
        is_anamorphic: Some(false),
        ref_frames: Some(1),
        codec_tag: Some("tx3g".to_owned()),
        comment: Some("external subtitle".to_owned()),
        nal_length_size: Some("0".to_owned()),
        is_avc: Some(true),
        title: Some("English".to_owned()),
        time_base: Some("1/1000000".to_owned()),
        codec_time_base: Some("1/1000".to_owned()),
        color_primaries: Some("bt709".to_owned()),
        color_space: Some("bt709".to_owned()),
        color_transfer: Some("bt709".to_owned()),
        dv_version_major: Some(1),
        dv_version_minor: Some(0),
        dv_profile: Some(8),
        dv_level: Some(6),
        rpu_present_flag: Some(1),
        el_present_flag: Some(0),
        bl_present_flag: Some(1),
        dv_bl_signal_compatibility_id: Some(1),
        is_hearing_impaired: true,
        rotation: Some(90),
        hdr10_plus_present_flag: Some(true),
        ..MediaStream::default()
    };

    let persisted = mapper.to_persisted(&stream);

    assert_eq!(persisted.stream_index, -1);
    assert_eq!(persisted.stream_type, PersistedMediaStreamType::Subtitle);
    assert_eq!(persisted.path.as_deref(), Some("virtual:movie.eng.srt"));
    assert_eq!(persisted.is_interlaced, Some(false));
    assert_eq!(persisted.is_hearing_impaired, Some(true));
    assert!(persisted.is_default);
    assert!(persisted.is_forced);
    assert!(persisted.is_external);
    assert!((persisted.level.unwrap() - 4.1).abs() < 0.0001);
    assert_eq!(persisted.is_avc, Some(true));
    assert_eq!(persisted.color_primaries.as_deref(), Some("bt709"));
    assert_eq!(persisted.color_space.as_deref(), Some("bt709"));
    assert_eq!(persisted.color_transfer.as_deref(), Some("bt709"));
    assert_eq!(persisted.dv_bl_signal_compatibility_id, Some(1));
}

fn minimal_persisted(
    stream_index: i32,
    stream_type: PersistedMediaStreamType,
) -> PersistedMediaStream {
    PersistedMediaStream {
        stream_index,
        stream_type,
        codec: None,
        language: None,
        channel_layout: None,
        profile: None,
        aspect_ratio: None,
        path: None,
        is_interlaced: None,
        bit_rate: None,
        channels: None,
        sample_rate: None,
        is_default: false,
        is_forced: false,
        is_external: false,
        is_original: false,
        height: None,
        width: None,
        average_frame_rate: None,
        real_frame_rate: None,
        level: None,
        pixel_format: None,
        bit_depth: None,
        is_anamorphic: None,
        ref_frames: None,
        codec_tag: None,
        comment: None,
        nal_length_size: None,
        is_avc: None,
        title: None,
        time_base: None,
        codec_time_base: None,
        color_primaries: None,
        color_space: None,
        color_transfer: None,
        dv_version_major: None,
        dv_version_minor: None,
        dv_profile: None,
        dv_level: None,
        rpu_present_flag: None,
        el_present_flag: None,
        bl_present_flag: None,
        dv_bl_signal_compatibility_id: None,
        is_hearing_impaired: None,
        rotation: None,
        hdr10_plus_present_flag: None,
    }
}
