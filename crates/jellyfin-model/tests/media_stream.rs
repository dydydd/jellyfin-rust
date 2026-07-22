use jellyfin_model::{
    AudioSpatialFormat, MediaStream, MediaStreamType, VideoRange, VideoRangeType,
};

#[test]
fn display_title_matches_official_matrix() {
    let cases = [
        (
            "English - Und - ASS",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                title: Some("English".into()),
                language: Some(String::new()),
                codec: Some("ASS".into()),
                ..MediaStream::default()
            },
        ),
        (
            "English - Und",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                title: Some("English".into()),
                language: Some(String::new()),
                codec: Some(String::new()),
                ..MediaStream::default()
            },
        ),
        (
            "English",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                title: Some("English".into()),
                language: Some("EN".into()),
                codec: Some(String::new()),
                ..MediaStream::default()
            },
        ),
        (
            "English - Default - Forced - SRT",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                title: Some("English".into()),
                language: Some("EN".into()),
                is_forced: true,
                is_default: true,
                codec: Some("SRT".into()),
                ..MediaStream::default()
            },
        ),
        (
            "Title - EN - Default - Forced - SRT - External",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                title: Some("Title".into()),
                language: Some("EN".into()),
                is_forced: true,
                is_default: true,
                codec: Some("SRT".into()),
                is_external: true,
                ..MediaStream::default()
            },
        ),
        (
            "Und",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                ..MediaStream::default()
            },
        ),
        (
            "Title - EN - Hearing Impaired - Default - Forced - SRT",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                title: Some("Title".into()),
                language: Some("EN".into()),
                is_forced: true,
                is_default: true,
                is_hearing_impaired: true,
                codec: Some("SRT".into()),
                ..MediaStream::default()
            },
        ),
        (
            "Title - AAC - Default - External",
            MediaStream {
                stream_type: MediaStreamType::Audio,
                title: Some("Title".into()),
                is_default: true,
                codec: Some("AAC".into()),
                is_external: true,
                ..MediaStream::default()
            },
        ),
        (
            "Chinese (Simplified) - SRT",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                language: Some("zh-CN".into()),
                localized_language: Some("Chinese (Simplified)".into()),
                codec: Some("SRT".into()),
                ..MediaStream::default()
            },
        ),
        (
            "Japanese - AAC - Stereo",
            MediaStream {
                stream_type: MediaStreamType::Audio,
                language: Some("jpn".into()),
                localized_language: Some("Japanese".into()),
                codec: Some("AAC".into()),
                channel_layout: Some("stereo".into()),
                ..MediaStream::default()
            },
        ),
        (
            "Eng - ASS",
            MediaStream {
                stream_type: MediaStreamType::Subtitle,
                language: Some("eng".into()),
                codec: Some("ASS".into()),
                ..MediaStream::default()
            },
        ),
    ];

    assert_eq!(cases.len(), 11);
    for (expected, stream) in cases {
        assert_eq!(stream.display_title().as_deref(), Some(expected));
    }
}

#[test]
fn resolution_text_matches_official_matrix() {
    let cases = [
        (None, None, false, None),
        (None, Some(0), false, None),
        (Some(0), None, false, None),
        (Some(256), Some(144), false, Some("144p")),
        (Some(256), Some(144), true, Some("144i")),
        (Some(426), Some(240), false, Some("240p")),
        (Some(426), Some(240), true, Some("240i")),
        (Some(640), Some(360), false, Some("360p")),
        (Some(640), Some(360), true, Some("360i")),
        (Some(854), Some(480), false, Some("480p")),
        (Some(854), Some(480), true, Some("480i")),
        (Some(960), Some(540), false, Some("540p")),
        (Some(960), Some(540), true, Some("540i")),
        (Some(1024), Some(576), false, Some("576p")),
        (Some(1024), Some(576), true, Some("576i")),
        (Some(1280), Some(720), false, Some("720p")),
        (Some(1280), Some(720), true, Some("720i")),
        (Some(2560), Some(1080), false, Some("1080p")),
        (Some(2560), Some(1080), true, Some("1080i")),
        (Some(4096), Some(3072), false, Some("4K")),
        (Some(8192), Some(6144), false, Some("8K")),
        (Some(512), Some(384), false, Some("384p")),
        (Some(576), Some(336), false, Some("360p")),
        (Some(576), Some(336), true, Some("360i")),
        (Some(624), Some(352), false, Some("360p")),
        (Some(640), Some(352), false, Some("360p")),
        (Some(640), Some(480), false, Some("480p")),
        (Some(704), Some(396), false, Some("404p")),
        (Some(720), Some(404), false, Some("404p")),
        (Some(720), Some(480), false, Some("480p")),
        (Some(720), Some(576), false, Some("576p")),
        (Some(768), Some(576), false, Some("576p")),
        (Some(960), Some(544), false, Some("540p")),
        (Some(960), Some(544), true, Some("540i")),
        (Some(960), Some(720), false, Some("720p")),
        (Some(1280), Some(528), false, Some("720p")),
        (Some(1280), Some(532), false, Some("720p")),
        (Some(1280), Some(534), false, Some("720p")),
        (Some(1280), Some(536), false, Some("720p")),
        (Some(1280), Some(544), false, Some("720p")),
        (Some(1280), Some(690), false, Some("720p")),
        (Some(1280), Some(694), false, Some("720p")),
        (Some(1280), Some(696), false, Some("720p")),
        (Some(1280), Some(716), false, Some("720p")),
        (Some(1280), Some(718), false, Some("720p")),
        (Some(1920), Some(1080), false, Some("1080p")),
        (Some(1440), Some(1070), false, Some("1080p")),
        (Some(1440), Some(1072), false, Some("1080p")),
        (Some(1440), Some(1080), false, Some("1080p")),
        (Some(1440), Some(1440), false, Some("1080p")),
        (Some(1912), Some(792), false, Some("1080p")),
        (Some(1916), Some(1076), false, Some("1080p")),
        (Some(1918), Some(1080), false, Some("1080p")),
        (Some(1920), Some(796), false, Some("1080p")),
        (Some(1920), Some(800), false, Some("1080p")),
        (Some(1920), Some(802), false, Some("1080p")),
        (Some(1920), Some(804), false, Some("1080p")),
        (Some(1920), Some(808), false, Some("1080p")),
        (Some(1920), Some(816), false, Some("1080p")),
        (Some(1920), Some(856), false, Some("1080p")),
        (Some(1920), Some(960), false, Some("1080p")),
        (Some(1920), Some(1024), false, Some("1080p")),
        (Some(1920), Some(1040), false, Some("1080p")),
        (Some(1920), Some(1070), false, Some("1080p")),
        (Some(1920), Some(1072), false, Some("1080p")),
        (Some(1920), Some(1440), false, Some("1080p")),
        (Some(3840), Some(1600), false, Some("4K")),
        (Some(3840), Some(1606), false, Some("4K")),
        (Some(3840), Some(1608), false, Some("4K")),
        (Some(3840), Some(2160), false, Some("4K")),
        (Some(4090), Some(3070), false, Some("4K")),
        (Some(7680), Some(4320), false, Some("8K")),
        (Some(8190), Some(6140), false, Some("8K")),
    ];

    assert_eq!(cases.len(), 73);
    for (width, height, interlaced, expected) in cases {
        let stream = MediaStream {
            width,
            height,
            is_interlaced: interlaced,
            ..MediaStream::default()
        };
        assert_eq!(
            stream.get_resolution_text(),
            expected,
            "{width:?}x{height:?}"
        );
    }
}

#[test]
fn audio_title_codec_profile_language_and_spatial_rules_match_upstream() {
    let stream = MediaStream {
        stream_type: MediaStreamType::Audio,
        language: Some("und".into()),
        codec: Some("eac3".into()),
        profile: Some("lc".into()),
        channels: Some(6),
        is_original: true,
        ..MediaStream::default()
    };
    assert_eq!(
        stream.display_title().as_deref(),
        Some("Dolby Digital+ - 6 ch - Original")
    );

    let atmos = MediaStream {
        stream_type: MediaStreamType::Audio,
        profile: Some("Dolby Atmos / Dolby Digital+".into()),
        ..MediaStream::default()
    };
    assert_eq!(atmos.audio_spatial_format(), AudioSpatialFormat::DolbyAtmos);

    let dts_x = MediaStream {
        stream_type: MediaStreamType::Audio,
        profile: Some("DTS:X".into()),
        ..MediaStream::default()
    };
    assert_eq!(dts_x.audio_spatial_format(), AudioSpatialFormat::DtsX);
}

#[test]
fn video_title_range_and_dolby_vision_rules_match_upstream() {
    let hdr10 = MediaStream {
        stream_type: MediaStreamType::Video,
        width: Some(1920),
        height: Some(1080),
        codec: Some("hevc".into()),
        color_transfer: Some("smpte2084".into()),
        ..MediaStream::default()
    };
    assert_eq!(hdr10.video_range(), VideoRange::Hdr);
    assert_eq!(hdr10.video_range_type(), VideoRangeType::Hdr10);
    assert_eq!(hdr10.display_title().as_deref(), Some("1080p HEVC HDR"));

    let dovi = MediaStream {
        stream_type: MediaStreamType::Video,
        dv_profile: Some(8),
        rpu_present_flag: Some(1),
        bl_present_flag: Some(1),
        dv_bl_signal_compatibility_id: Some(1),
        ..MediaStream::default()
    };
    assert_eq!(
        dovi.video_dovi_title().as_deref(),
        Some("Dolby Vision Profile 8.1 (HDR10)")
    );
    assert_eq!(dovi.video_range_type(), VideoRangeType::DoviWithHdr10);
}

#[test]
fn reference_frame_rate_prefers_realistic_average() {
    let normal = MediaStream {
        average_frame_rate: Some(23.976),
        real_frame_rate: Some(24.0),
        ..MediaStream::default()
    };
    assert_eq!(normal.reference_frame_rate(), Some(23.976));

    let unrealistic = MediaStream {
        average_frame_rate: Some(1000.0),
        real_frame_rate: Some(24.0),
        ..MediaStream::default()
    };
    assert_eq!(unrealistic.reference_frame_rate(), Some(24.0));
}

#[test]
fn subtitle_codec_classification_and_conversion_match_upstream() {
    assert!(MediaStream::is_text_format(Some("microdvd/dvdsub")));
    assert!(MediaStream::is_text_format(Some("srt")));
    assert!(!MediaStream::is_text_format(Some("pgssub")));
    assert!(MediaStream::is_pgs_format(Some("SUP")));
    assert!(MediaStream::is_vobsub_format(Some("dvdsub")));

    let srt = MediaStream {
        stream_type: MediaStreamType::Subtitle,
        codec: Some("srt".into()),
        ..MediaStream::default()
    };
    assert!(srt.is_text_subtitle_stream());
    assert!(srt.is_extractable_subtitle_stream());
    assert!(srt.supports_subtitle_conversion_to("vtt"));
    assert!(!srt.supports_subtitle_conversion_to("ass"));

    let ass = MediaStream {
        stream_type: MediaStreamType::Subtitle,
        codec: Some("ass".into()),
        ..MediaStream::default()
    };
    assert!(!ass.supports_subtitle_conversion_to("srt"));
}
