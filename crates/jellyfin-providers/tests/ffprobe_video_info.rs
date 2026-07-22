use std::{cell::RefCell, collections::VecDeque, convert::Infallible};

use jellyfin_model::{MediaProtocol, MediaStream, MediaStreamType, VideoType};
use jellyfin_providers::media_info::{
    BlurayDiscInfo, ChapterInfo, DummyChapterError, EmbeddedSubtitleMode, FfprobeVideoInfo,
    FfprobeVideoInfoCapability, IsoType, VideoMediaInfo, VideoMediaInfoRequest, VideoProbeItem,
    VideoProbeMetadata, VideoProbeSkipReason, apply_media_info_metadata, merge_bluray_info,
    normalize_chapter_names, normalize_video_streams,
};

const TICKS_PER_MINUTE: i64 = 60 * 10_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct OwnedRequest {
    path: String,
    protocol: MediaProtocol,
    video_type: VideoType,
    iso_type: Option<IsoType>,
    extract_chapters: bool,
}

struct FixtureCapability {
    path_protocol: MediaProtocol,
    vob_files: Vec<String>,
    bluray_info: Option<BlurayDiscInfo>,
    media_info: RefCell<VecDeque<VideoMediaInfo>>,
    requests: RefCell<Vec<OwnedRequest>>,
    protocol_paths: RefCell<Vec<String>>,
}

impl FixtureCapability {
    fn new(media_info: Vec<VideoMediaInfo>) -> Self {
        Self {
            path_protocol: MediaProtocol::File,
            vob_files: Vec::new(),
            bluray_info: None,
            media_info: RefCell::new(media_info.into()),
            requests: RefCell::new(Vec::new()),
            protocol_paths: RefCell::new(Vec::new()),
        }
    }
}

impl FfprobeVideoInfoCapability for FixtureCapability {
    type Error = Infallible;

    fn get_path_protocol(&self, path: &str) -> MediaProtocol {
        self.protocol_paths.borrow_mut().push(path.to_owned());
        self.path_protocol
    }

    fn get_primary_playlist_vob_files(&self, _path: &str) -> Result<Vec<String>, Self::Error> {
        Ok(self.vob_files.clone())
    }

    fn get_bluray_info(&self, _path: &str) -> Option<BlurayDiscInfo> {
        self.bluray_info.clone()
    }

    fn get_media_info(
        &self,
        request: VideoMediaInfoRequest<'_>,
    ) -> Result<VideoMediaInfo, Self::Error> {
        self.requests.borrow_mut().push(OwnedRequest {
            path: request.path.to_owned(),
            protocol: request.protocol,
            video_type: request.video_type,
            iso_type: request.iso_type,
            extract_chapters: request.extract_chapters,
        });
        Ok(self.media_info.borrow_mut().pop_front().unwrap_or_default())
    }
}

#[test]
fn create_dummy_chapters_invalid_runtime_returns_error() {
    let processor = FfprobeVideoInfo::default();
    for runtime in [-1, i64::MIN, i64::MAX] {
        assert_eq!(
            processor.create_dummy_chapters(Some(runtime)),
            Err(DummyChapterError::InvalidRuntime(runtime))
        );
    }
}

#[test]
fn create_dummy_chapters_valid_runtime_has_official_count() {
    let processor = FfprobeVideoInfo::default();
    for (runtime, expected_count) in [
        (None, 0),
        (Some(0), 0),
        (Some(1), 1),
        (Some(TICKS_PER_MINUTE * 3), 1),
        (Some(TICKS_PER_MINUTE * 5), 1),
        (Some(TICKS_PER_MINUTE * 5 + 1), 1),
        (Some(TICKS_PER_MINUTE * 50), 10),
    ] {
        assert_eq!(
            processor.create_dummy_chapters(runtime).unwrap().len(),
            expected_count,
            "runtime: {runtime:?}"
        );
    }
}

#[test]
fn create_dummy_chapters_never_starts_beyond_runtime() {
    let processor = FfprobeVideoInfo::default();
    for runtime in [
        1,
        TICKS_PER_MINUTE * 3,
        TICKS_PER_MINUTE * 5,
        TICKS_PER_MINUTE * 5 + 1,
        TICKS_PER_MINUTE * 50 + 1,
    ] {
        let chapters = processor.create_dummy_chapters(Some(runtime)).unwrap();
        assert!(
            chapters
                .iter()
                .all(|chapter| chapter.start_position_ticks < runtime)
        );
    }
}

#[test]
fn selects_file_and_shortcut_paths_protocols_and_probe_gate() {
    let processor = FfprobeVideoInfo::default();
    let mut capability = FixtureCapability::new(vec![VideoMediaInfo::default(); 2]);
    capability.path_protocol = MediaProtocol::Http;
    let iso_item = VideoProbeItem {
        path: "/media/movie.iso",
        protocol: Some(MediaProtocol::Ftp),
        video_type: VideoType::Iso,
        iso_type: Some(IsoType::Dvd),
        is_shortcut: false,
        shortcut_path: None,
    };
    processor.probe(iso_item, false, &capability).unwrap();

    let shortcut_item = VideoProbeItem {
        path: "/media/movie.strm",
        protocol: Some(MediaProtocol::File),
        video_type: VideoType::VideoFile,
        iso_type: None,
        is_shortcut: true,
        shortcut_path: Some("https://example.com/movie.mkv"),
    };
    let skipped = processor.probe(shortcut_item, false, &capability).unwrap();
    assert_eq!(
        skipped.skip_reason,
        Some(VideoProbeSkipReason::RemoteShortcutDisabled)
    );
    processor.probe(shortcut_item, true, &capability).unwrap();

    assert_eq!(
        *capability.requests.borrow(),
        [
            OwnedRequest {
                path: "/media/movie.iso".to_owned(),
                protocol: MediaProtocol::Ftp,
                video_type: VideoType::Iso,
                iso_type: Some(IsoType::Dvd),
                extract_chapters: true,
            },
            OwnedRequest {
                path: "https://example.com/movie.mkv".to_owned(),
                protocol: MediaProtocol::Http,
                video_type: VideoType::VideoFile,
                iso_type: None,
                extract_chapters: true,
            },
        ]
    );
    assert_eq!(
        *capability.protocol_paths.borrow(),
        ["https://example.com/movie.mkv"]
    );
}

#[test]
fn dvd_probe_skips_empty_playlists_and_sums_vob_runtime() {
    let processor = FfprobeVideoInfo::default();
    let item = disc_item("/disc/VIDEO_TS", VideoType::Dvd);
    let empty = FixtureCapability::new(Vec::new());
    let outcome = processor.probe(item, false, &empty).unwrap();
    assert_eq!(
        outcome.skip_reason,
        Some(VideoProbeSkipReason::NoPlayableDvdFiles)
    );
    assert!(empty.requests.borrow().is_empty());

    let mut capability = FixtureCapability::new(vec![
        info_with_runtime(Some(10)),
        info_with_runtime(Some(20)),
        info_with_runtime(Some(30)),
    ]);
    capability.vob_files = vec!["VTS_01_1.VOB", "VTS_01_2.VOB", "VTS_01_3.VOB"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();
    let outcome = processor.probe(item, false, &capability).unwrap();
    assert_eq!(outcome.media_info.unwrap().run_time_ticks, Some(60));
    assert_eq!(capability.requests.borrow().len(), 3);
    assert!(capability.requests.borrow().iter().all(|request| {
        request.protocol == MediaProtocol::File
            && request.video_type == VideoType::VideoFile
            && request.iso_type.is_none()
    }));
}

#[test]
fn bluray_probe_requires_playlist_and_probes_first_file() {
    let processor = FfprobeVideoInfo::default();
    let item = disc_item("/disc/BDMV", VideoType::BluRay);
    let missing = FixtureCapability::new(Vec::new());
    assert_eq!(
        processor.probe(item, false, &missing).unwrap().skip_reason,
        Some(VideoProbeSkipReason::NoPlayableBlurayFiles)
    );

    let mut empty = FixtureCapability::new(Vec::new());
    empty.bluray_info = Some(BlurayDiscInfo::default());
    assert_eq!(
        processor.probe(item, false, &empty).unwrap().skip_reason,
        Some(VideoProbeSkipReason::NoPlayableBlurayFiles)
    );

    let mut capability = FixtureCapability::new(vec![info_with_runtime(Some(42))]);
    capability.bluray_info = Some(BlurayDiscInfo {
        files: vec!["/disc/BDMV/STREAM/00001.m2ts".to_owned()],
        run_time_ticks: Some(100),
        ..BlurayDiscInfo::default()
    });
    let outcome = processor.probe(item, false, &capability).unwrap();
    assert_eq!(outcome.media_info.unwrap().run_time_ticks, Some(42));
    assert_eq!(outcome.bluray_info.unwrap().run_time_ticks, Some(100));
    assert_eq!(
        capability.requests.borrow()[0],
        OwnedRequest {
            path: "/disc/BDMV/STREAM/00001.m2ts".to_owned(),
            protocol: MediaProtocol::File,
            video_type: VideoType::VideoFile,
            iso_type: None,
            extract_chapters: true,
        }
    );
}

#[test]
fn applies_probe_metadata_with_disc_size_rule() {
    let info = VideoMediaInfo {
        bitrate: Some(8_000_000),
        run_time_ticks: Some(123),
        container: Some("mkv".to_owned()),
        size: Some(456),
        ..VideoMediaInfo::default()
    };
    let mut file = VideoProbeMetadata {
        size: Some(999),
        ..VideoProbeMetadata::default()
    };
    apply_media_info_metadata(&mut file, VideoType::VideoFile, &info);
    assert_eq!(file.total_bitrate, Some(8_000_000));
    assert_eq!(file.run_time_ticks, Some(123));
    assert_eq!(file.container.as_deref(), Some("mkv"));
    assert_eq!(file.size, Some(999));

    let mut disc = VideoProbeMetadata::default();
    apply_media_info_metadata(&mut disc, VideoType::BluRay, &info);
    assert_eq!(disc.size, Some(456));
}

#[test]
fn normalizes_stream_order_indices_paths_and_item_fields() {
    let external_subtitles = vec![
        external_stream(MediaStreamType::Subtitle, "movie.en.srt"),
        external_stream(MediaStreamType::Subtitle, "movie.en.srt"),
    ];
    let external_audio = vec![external_stream(MediaStreamType::Audio, "movie.en.flac")];
    let embedded = vec![
        MediaStream {
            stream_type: MediaStreamType::Video,
            width: Some(1920),
            height: Some(1080),
            ..MediaStream::default()
        },
        subtitle("srt"),
    ];
    let result = normalize_video_streams(
        embedded,
        external_audio,
        external_subtitles,
        EmbeddedSubtitleMode::AllowAll,
    );

    assert_eq!(
        result
            .streams
            .iter()
            .map(|stream| stream.stream_type)
            .collect::<Vec<_>>(),
        [
            MediaStreamType::Subtitle,
            MediaStreamType::Subtitle,
            MediaStreamType::Audio,
            MediaStreamType::Video,
            MediaStreamType::Subtitle,
        ]
    );
    assert_eq!(
        result
            .streams
            .iter()
            .map(|stream| stream.index)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    assert_eq!(result.width, 1920);
    assert_eq!(result.height, 1080);
    assert_eq!(result.default_video_stream_index, Some(3));
    assert!(result.has_subtitles);
    assert_eq!(result.audio_files, ["movie.en.flac"]);
    assert_eq!(result.subtitle_files, ["movie.en.srt"]);
}

#[test]
fn filters_only_embedded_subtitles_by_configured_representation() {
    for (mode, keeps_text, keeps_image) in [
        (EmbeddedSubtitleMode::AllowAll, true, true),
        (EmbeddedSubtitleMode::AllowText, true, false),
        (EmbeddedSubtitleMode::AllowImage, false, true),
        (EmbeddedSubtitleMode::AllowNone, false, false),
    ] {
        let streams = normalize_video_streams(
            vec![subtitle("srt"), subtitle("hdmv_pgs_subtitle")],
            Vec::new(),
            vec![external_stream(MediaStreamType::Subtitle, "external.srt")],
            mode,
        )
        .streams;
        assert_eq!(
            streams
                .iter()
                .any(|stream| stream.codec.as_deref() == Some("srt") && !stream.is_external),
            keeps_text
        );
        assert_eq!(
            streams.iter().any(|stream| {
                stream.codec.as_deref() == Some("hdmv_pgs_subtitle") && !stream.is_external
            }),
            keeps_image
        );
        assert!(streams[0].is_external);
        assert_eq!(streams[0].index, 0);
    }
}

#[test]
fn merges_bluray_streams_runtime_chapters_and_ffprobe_video_fields() {
    let external = external_stream(MediaStreamType::Audio, "commentary.flac");
    let ffprobe_video = MediaStream {
        stream_type: MediaStreamType::Video,
        codec: Some("hevc".to_owned()),
        bit_rate: Some(5_000_000),
        width: Some(3840),
        height: Some(2160),
        color_transfer: Some("smpte2084".to_owned()),
        bit_depth: Some(10),
        ..MediaStream::default()
    };
    let mut streams = vec![external.clone(), ffprobe_video, audio_stream()];
    let mut metadata = VideoProbeMetadata {
        run_time_ticks: Some(50),
        ..VideoProbeMetadata::default()
    };
    let mut chapters = vec![ChapterInfo {
        start_position_ticks: 1,
        name: Some("old".to_owned()),
    }];
    let bluray = BlurayDiscInfo {
        media_streams: vec![
            MediaStream {
                stream_type: MediaStreamType::Video,
                codec: Some("mpeg2video".to_owned()),
                bit_rate: Some(0),
                width: None,
                height: Some(0),
                ..MediaStream::default()
            },
            audio_stream(),
        ],
        run_time_ticks: Some(100),
        chapters_seconds: Some(vec![0.0, 12.5]),
        ..BlurayDiscInfo::default()
    };

    merge_bluray_info(&mut metadata, &mut chapters, &mut streams, &bluray);
    assert_eq!(metadata.run_time_ticks, Some(100));
    assert_eq!(
        chapters
            .iter()
            .map(|chapter| chapter.start_position_ticks)
            .collect::<Vec<_>>(),
        [0, 125_000_000]
    );
    assert_eq!(streams.len(), 3);
    assert_eq!(streams[0].path, external.path);
    assert_eq!(
        streams
            .iter()
            .map(|stream| stream.index)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let video = &streams[1];
    assert_eq!(video.codec.as_deref(), Some("hevc"));
    assert_eq!(video.bit_rate, Some(5_000_000));
    assert_eq!(video.width, Some(3840));
    assert_eq!(video.height, Some(2160));
    assert_eq!(video.color_transfer.as_deref(), Some("smpte2084"));
    assert_eq!(video.bit_depth, Some(10));
}

#[test]
fn normalizes_blank_and_timestamp_chapter_names() {
    let mut chapters = vec![
        ChapterInfo::default(),
        ChapterInfo {
            name: Some("  ".to_owned()),
            ..ChapterInfo::default()
        },
        ChapterInfo {
            name: Some("00:10:00".to_owned()),
            ..ChapterInfo::default()
        },
        ChapterInfo {
            name: Some("Opening".to_owned()),
            ..ChapterInfo::default()
        },
    ];
    normalize_chapter_names(&mut chapters, "Chapter {0}");
    assert_eq!(
        chapters
            .iter()
            .map(|chapter| chapter.name.as_deref())
            .collect::<Vec<_>>(),
        [
            Some("Chapter 1"),
            Some("Chapter 2"),
            Some("Chapter 3"),
            Some("Opening"),
        ]
    );
}

fn disc_item(path: &str, video_type: VideoType) -> VideoProbeItem<'_> {
    VideoProbeItem {
        path,
        protocol: None,
        video_type,
        iso_type: None,
        is_shortcut: false,
        shortcut_path: None,
    }
}

fn info_with_runtime(run_time_ticks: Option<i64>) -> VideoMediaInfo {
    VideoMediaInfo {
        run_time_ticks,
        ..VideoMediaInfo::default()
    }
}

fn external_stream(stream_type: MediaStreamType, path: &str) -> MediaStream {
    MediaStream {
        stream_type,
        path: Some(path.to_owned()),
        is_external: true,
        ..MediaStream::default()
    }
}

fn subtitle(codec: &str) -> MediaStream {
    MediaStream {
        stream_type: MediaStreamType::Subtitle,
        codec: Some(codec.to_owned()),
        ..MediaStream::default()
    }
}

fn audio_stream() -> MediaStream {
    MediaStream {
        stream_type: MediaStreamType::Audio,
        ..MediaStream::default()
    }
}
