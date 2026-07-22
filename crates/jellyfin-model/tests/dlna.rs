use jellyfin_model::{
    ContainerHelper, ContainerProfile, DlnaProfileType, MediaSourceInfo, MediaStreamProtocol,
    PlayMethod, StreamInfo, SubtitleDeliveryMethod, TranscodeReason, TranscodeSeekInfo, VideoType,
};
use uuid::Uuid;

#[test]
fn empty_container_profile_accepts_every_container() {
    let profile = ContainerProfile::default();

    for container in ["", "mp4"] {
        assert!(profile.contains_container(container, false));
    }
}

#[test]
fn string_container_matching_matches_official_cases() {
    let matching = [
        ("mp3,mpeg", "mp3"),
        ("mp3,mpeg,avi", "mp3,avi"),
        ("-mp3,mpeg", "avi"),
        ("-mp3,mpeg,avi", "mp4,jpg"),
    ];
    for (profile, input) in matching {
        assert!(ContainerHelper::contains_container(
            Some(profile),
            Some(input)
        ));
    }

    let not_matching = [
        ("mp3,mpeg", Some("avi")),
        ("mp3,mpeg,avi", Some("mp4,jpg")),
        ("mp3,mpeg", None),
        ("mp3,mpeg", Some("")),
        ("-mp3,mpeg", Some("mp3")),
        ("-mp3,mpeg,avi", Some("mpeg,avi")),
        (",mp3,", Some(",avi,")),
        ("-,mp3,", Some(",mp3,")),
    ];
    for (profile, input) in not_matching {
        assert!(!ContainerHelper::contains_container(Some(profile), input));
        if let Some(input) = input {
            assert!(!ContainerHelper::contains_container_span(
                Some(profile),
                input
            ));
        }
    }
}

#[test]
fn span_and_list_container_matching_matches_official_cases() {
    for (profile, input) in [
        ("mp3,mpeg", "mp3"),
        ("mp3,mpeg,avi", "mp3,avi"),
        ("-mp3,mpeg", "avi"),
        ("-mp3,mpeg,avi", "mp4,jpg"),
    ] {
        assert!(ContainerHelper::contains_container_span(
            Some(profile),
            input
        ));
    }

    let list_cases = [
        (&["mp3", "mpeg"][..], false, "mpeg", true),
        (&["mp3", "mpeg", "avi"], false, "avi", true),
        (&["mp3", "", "avi"], false, "mp3", true),
        (&["mp3", "mpeg"], true, "avi", true),
        (&["mp3", "mpeg", "avi"], true, "mkv", true),
        (&["mp3", "", "avi"], true, "", true),
        (&["mp3", "mpeg"], false, "avi", false),
        (&["mp3", "mpeg", "avi"], false, "mkv", false),
        (&["mp3", "", "avi"], false, "", false),
        (&["mp3", "mpeg"], true, "mpeg", false),
        (&["mp3", "mpeg", "avi"], true, "mp3", false),
        (&["mp3", "", "avi"], true, "avi", false),
    ];
    for (profiles, is_negative, input, expected) in list_cases {
        let profiles = profiles
            .iter()
            .map(|profile| (*profile).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            ContainerHelper::contains_container_list(Some(&profiles), is_negative, input),
            expected
        );
    }
}

#[test]
fn container_profile_uses_hls_subcontainer_only_when_requested() {
    let profile = ContainerProfile {
        container: Some("HLS".to_owned()),
        sub_container: Some("mpegts".to_owned()),
        ..ContainerProfile::default()
    };

    assert!(profile.contains_container("hls", false));
    assert!(profile.contains_container("mpegts", true));
    assert!(!profile.contains_container("hls", true));
}

#[test]
fn blank_stream_urls_match_legacy_shape() {
    for media_type in [
        DlnaProfileType::Audio,
        DlnaProfileType::Video,
        DlnaProfileType::Photo,
    ] {
        let info = StreamInfo::new(Uuid::nil(), media_type);
        assert_eq!(
            info.to_url(Some("/test/"), Some("123"), None),
            legacy_to_url(&info, "/test/", Some("123"))
        );
    }
}

#[test]
fn url_includes_all_supported_parameters_in_official_order() {
    let mut info = StreamInfo::new(
        Uuid::parse_str("e742c26f-67c9-475d-b51b-21e194d39f61").unwrap(),
        DlnaProfileType::Video,
    );
    info.container = Some("ts".to_owned());
    info.sub_protocol = MediaStreamProtocol::Hls;
    info.device_profile_id = Some("profile".to_owned());
    info.device_id = Some("device".to_owned());
    info.play_method = PlayMethod::Transcode;
    info.audio_codecs = vec!["aac".to_owned(), "ac3".to_owned()];
    info.video_codecs = vec!["h264".to_owned()];
    info.audio_stream_index = Some(2);
    info.subtitle_stream_index = Some(3);
    info.subtitle_delivery_method = SubtitleDeliveryMethod::Embed;
    info.subtitle_codecs = vec!["ass".to_owned()];
    info.segment_length = Some(6);
    info.min_segments = Some(2);
    info.require_non_anamorphic = true;
    info.require_avc = true;
    info.transcode_seek_info = TranscodeSeekInfo::Bytes;
    info.media_source = Some(MediaSourceInfo {
        id: Some("source".to_owned()),
        live_stream_id: Some("live".to_owned()),
        etag: Some("etag".to_owned()),
        video_type: Some(VideoType::VideoFile),
    });
    info.transcode_reasons =
        TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED | TranscodeReason::AUDIO_CODEC_NOT_SUPPORTED;
    info.set_option("VideoProfile", "high 10");

    assert_eq!(
        info.to_url(Some("http://localhost/"), Some("token"), Some("&extra=1")),
        "http://localhost/videos/e742c26f-67c9-475d-b51b-21e194d39f61/master.m3u8\
?DeviceProfileId=profile&DeviceId=device&MediaSourceId=source&VideoCodec=h264\
&AudioCodec=aac,ac3&AudioStreamIndex=2&SubtitleStreamIndex=3&SegmentContainer=ts\
&SegmentLength=6&MinSegments=2&ApiKey=token&LiveStreamId=live\
&RequireNonAnamorphic=True&TranscodeSeekInfo=Bytes&RequireAvc=true\
&EnableAudioVbrEncoding=false&Tag=etag&SubtitleMethod=Embed&SubtitleCodec=ass\
&VideoProfile=high10&TranscodeReasons=VideoCodecNotSupported,AudioCodecNotSupported&extra=1"
    );
}

#[test]
fn disc_sources_are_never_direct_streams() {
    for video_type in [VideoType::Dvd, VideoType::BluRay] {
        let mut info = StreamInfo::default();
        info.play_method = PlayMethod::DirectPlay;
        info.media_source = Some(MediaSourceInfo {
            video_type: Some(video_type),
            ..MediaSourceInfo::default()
        });

        assert!(!info.is_direct_stream());
    }
}

#[test]
fn fuzzy_urls_match_independent_legacy_builder() {
    let mut random = TestRandom::new(298_347_823);
    for _ in 0..100_000 {
        let mut info = StreamInfo::new(random.uuid(), random.profile_type());
        info.play_method = random.play_method();
        info.container = random.optional_string(10);
        info.sub_protocol = random.protocol();
        info.start_position_ticks = random.next_u64() as i64;
        info.segment_length = random.optional_i32();
        info.min_segments = random.optional_i32();
        info.require_avc = random.boolean();
        info.require_non_anamorphic = random.boolean();
        info.copy_timestamps = random.boolean();
        info.enable_mpegts_m2ts_mode = random.boolean();
        info.enable_subtitles_in_manifest = random.boolean();
        info.audio_codecs = random.strings();
        info.video_codecs = random.strings();
        info.audio_stream_index = random.optional_i32();
        info.subtitle_stream_index = random.optional_i32();
        info.transcoding_max_audio_channels = random.optional_i32();
        info.audio_bitrate = random.optional_i32();
        info.audio_sample_rate = random.optional_i32();
        info.video_bitrate = random.optional_i32();
        info.max_width = random.optional_i32();
        info.max_height = random.optional_i32();
        info.max_framerate = random.optional_f32();
        info.device_profile_id = random.optional_string(12);
        info.device_id = random.optional_string(12);
        info.transcode_seek_info = if random.boolean() {
            TranscodeSeekInfo::Auto
        } else {
            TranscodeSeekInfo::Bytes
        };
        info.estimate_content_length = random.boolean();
        // The official reflection-based fuzz test leaves IReadOnlyList properties empty.
        info.subtitle_codecs = Vec::new();
        info.subtitle_delivery_method = random.subtitle_method();
        info.play_session_id = random.optional_string(12);
        info.enable_audio_vbr_encoding = random.boolean();
        info.always_burn_in_subtitle_when_transcoding = random.boolean();

        let current = info.to_url(Some("/test/"), Some("123"), None);
        let legacy = legacy_to_url(&info, "/test/", Some("123"));
        assert!(current.eq_ignore_ascii_case(&legacy), "{current}\n{legacy}");
    }
}

fn legacy_to_url(info: &StreamInfo, base_url: &str, access_token: Option<&str>) -> String {
    let mut params = Vec::new();
    push_legacy(
        &mut params,
        "DeviceProfileId",
        info.device_profile_id.clone(),
    );
    push_legacy(&mut params, "DeviceId", info.device_id.clone());
    push_legacy(
        &mut params,
        "MediaSourceId",
        info.media_source_id().map(str::to_owned),
    );
    push_legacy(
        &mut params,
        "Static",
        Some(info.is_direct_stream().to_string()),
    );
    push_legacy(&mut params, "VideoCodec", join(&info.video_codecs));
    push_legacy(&mut params, "AudioCodec", join(&info.audio_codecs));
    push_legacy(
        &mut params,
        "AudioStreamIndex",
        info.audio_stream_index.map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "SubtitleStreamIndex",
        info.subtitle_stream_index
            .filter(|_| {
                info.always_burn_in_subtitle_when_transcoding
                    || info.subtitle_delivery_method != SubtitleDeliveryMethod::External
            })
            .map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "VideoBitrate",
        info.video_bitrate.map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "AudioBitrate",
        info.audio_bitrate.map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "AudioSampleRate",
        info.audio_sample_rate.map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "MaxFramerate",
        info.max_framerate.map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "MaxWidth",
        info.max_width.map(|value| value.to_string()),
    );
    push_legacy(
        &mut params,
        "MaxHeight",
        info.max_height.map(|value| value.to_string()),
    );

    if info.sub_protocol == MediaStreamProtocol::Hls {
        push_legacy(&mut params, "StartTimeTicks", None);
        push_legacy(&mut params, "SegmentContainer", info.container.clone());
        push_legacy(
            &mut params,
            "SegmentLength",
            info.segment_length.map(|value| value.to_string()),
        );
        push_legacy(
            &mut params,
            "MinSegments",
            info.min_segments.map(|value| value.to_string()),
        );
    } else {
        push_legacy(
            &mut params,
            "StartTimeTicks",
            Some(info.start_position_ticks.to_string()),
        );
    }
    push_legacy(&mut params, "PlaySessionId", info.play_session_id.clone());
    push_legacy(&mut params, "ApiKey", access_token.map(str::to_owned));
    push_legacy(
        &mut params,
        "LiveStreamId",
        info.media_source
            .as_ref()
            .and_then(|source| source.live_stream_id.clone()),
    );

    if !info.is_direct_stream() {
        if info.require_non_anamorphic {
            push_legacy(
                &mut params,
                "RequireNonAnamorphic",
                Some(info.require_non_anamorphic.to_string()),
            );
        }
        push_legacy(
            &mut params,
            "TranscodingMaxAudioChannels",
            info.transcoding_max_audio_channels
                .map(|value| value.to_string()),
        );
        if info.enable_subtitles_in_manifest {
            push_legacy(
                &mut params,
                "EnableSubtitlesInManifest",
                Some(info.enable_subtitles_in_manifest.to_string()),
            );
        }
        if info.enable_mpegts_m2ts_mode {
            push_legacy(
                &mut params,
                "EnableMpegtsM2TsMode",
                Some(info.enable_mpegts_m2ts_mode.to_string()),
            );
        }
        if info.estimate_content_length {
            push_legacy(
                &mut params,
                "EstimateContentLength",
                Some(info.estimate_content_length.to_string()),
            );
        }
        if info.transcode_seek_info != TranscodeSeekInfo::Auto {
            push_legacy(&mut params, "TranscodeSeekInfo", Some("bytes".to_owned()));
        }
        if info.copy_timestamps {
            push_legacy(
                &mut params,
                "CopyTimestamps",
                Some(info.copy_timestamps.to_string()),
            );
        }
        push_legacy(
            &mut params,
            "RequireAvc",
            Some(info.require_avc.to_string()),
        );
        push_legacy(
            &mut params,
            "EnableAudioVbrEncoding",
            Some(info.enable_audio_vbr_encoding.to_string()),
        );
    }

    push_legacy(
        &mut params,
        "Tag",
        info.media_source
            .as_ref()
            .and_then(|source| source.etag.clone()),
    );
    push_legacy(
        &mut params,
        "SubtitleCodec",
        (info.subtitle_stream_index.is_some()
            && info.subtitle_delivery_method == SubtitleDeliveryMethod::Embed)
            .then(|| info.subtitle_codecs.join(",")),
    );
    push_legacy(
        &mut params,
        "SubtitleMethod",
        (info.subtitle_stream_index.is_some()
            && info.subtitle_delivery_method != SubtitleDeliveryMethod::External)
            .then(|| format!("{:?}", info.subtitle_delivery_method)),
    );

    let query = params
        .into_iter()
        .filter(|(name, value)| {
            !value.is_empty()
                && !(name.eq_ignore_ascii_case("StartTimeTicks") && value == "0")
                && !(name.eq_ignore_ascii_case("SubtitleStreamIndex") && value == "-1")
                && !(name.eq_ignore_ascii_case("Static") && value.eq_ignore_ascii_case("false"))
        })
        .map(|(name, value)| format!("{name}={}", value.replace(' ', "%20")))
        .collect::<Vec<_>>()
        .join("&");

    let prefix = if info.media_type == DlnaProfileType::Audio {
        "audio"
    } else {
        "videos"
    };
    let path = if info.sub_protocol == MediaStreamProtocol::Hls {
        "master.m3u8".to_owned()
    } else {
        info.container
            .as_ref()
            .filter(|value| !value.is_empty())
            .map_or_else(
                || "stream".to_owned(),
                |container| format!("stream.{container}"),
            )
    };
    format!(
        "{}/{}/{}/{}?{}",
        base_url.trim_end_matches('/'),
        prefix,
        info.item_id,
        path,
        query
    )
}

fn push_legacy(
    params: &mut Vec<(&'static str, String)>,
    name: &'static str,
    value: Option<String>,
) {
    params.push((name, value.unwrap_or_default()));
}

fn join(values: &[String]) -> Option<String> {
    (!values.is_empty()).then(|| values.join(","))
}

struct TestRandom(u64);

impl TestRandom {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn boolean(&mut self) -> bool {
        self.next_u64() & 1 != 0
    }

    fn optional_i32(&mut self) -> Option<i32> {
        (self.next_u64() & 3 != 0).then(|| self.next_u64() as i32)
    }

    fn optional_f32(&mut self) -> Option<f32> {
        (self.next_u64() & 3 != 0).then(|| f32::from_bits(self.next_u64() as u32))
    }

    fn optional_string(&mut self, max_length: usize) -> Option<String> {
        (self.next_u64() & 3 != 0).then(|| self.string(max_length))
    }

    fn string(&mut self, max_length: usize) -> String {
        let length = self.next_u64() as usize % max_length;
        (0..length)
            .map(|_| (b'A' + (self.next_u64() % 32) as u8) as char)
            .collect()
    }

    fn strings(&mut self) -> Vec<String> {
        let length = self.next_u64() as usize % 4;
        (0..length).map(|_| self.string(8)).collect()
    }

    fn uuid(&mut self) -> Uuid {
        Uuid::from_u128(((self.next_u64() as u128) << 64) | self.next_u64() as u128)
    }

    fn profile_type(&mut self) -> DlnaProfileType {
        match self.next_u64() % 5 {
            0 => DlnaProfileType::Audio,
            1 => DlnaProfileType::Video,
            2 => DlnaProfileType::Photo,
            3 => DlnaProfileType::Subtitle,
            _ => DlnaProfileType::Lyric,
        }
    }

    fn play_method(&mut self) -> PlayMethod {
        match self.next_u64() % 3 {
            0 => PlayMethod::Transcode,
            1 => PlayMethod::DirectStream,
            _ => PlayMethod::DirectPlay,
        }
    }

    fn protocol(&mut self) -> MediaStreamProtocol {
        if self.boolean() {
            MediaStreamProtocol::Http
        } else {
            MediaStreamProtocol::Hls
        }
    }

    fn subtitle_method(&mut self) -> SubtitleDeliveryMethod {
        match self.next_u64() % 5 {
            0 => SubtitleDeliveryMethod::Encode,
            1 => SubtitleDeliveryMethod::Embed,
            2 => SubtitleDeliveryMethod::External,
            3 => SubtitleDeliveryMethod::Hls,
            _ => SubtitleDeliveryMethod::Drop,
        }
    }
}
