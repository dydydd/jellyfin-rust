use std::cell::RefCell;

use jellyfin_model::{
    ImageFormat, ImageType, MediaProtocol, MediaStream, MediaStreamType, VideoType,
};
use jellyfin_providers::media_info::{
    EmbeddedImageCacheKey, EmbeddedImageCapability, EmbeddedImageExtractionRequest,
    EmbeddedImageItem, EmbeddedImageItemKind, IsoType, Video3DFormat, VideoFrameExtractionRequest,
    VideoImageCapability, VideoImageProvider, VideoImageRequest,
};

const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Clone, Debug, PartialEq)]
struct OwnedFrameRequest {
    item_path: String,
    container: Option<String>,
    protocol: MediaProtocol,
    video_type: VideoType,
    iso_type: Option<IsoType>,
    stream: MediaStream,
    video_3d_format: Option<Video3DFormat>,
    offset_ticks: i64,
}

#[derive(Default)]
struct FixtureCapability {
    cached_path: Option<String>,
    error: Option<&'static str>,
    cache_keys: RefCell<Vec<EmbeddedImageCacheKey>>,
    requests: RefCell<Vec<OwnedFrameRequest>>,
}

impl EmbeddedImageCapability for FixtureCapability {
    type Error = &'static str;

    fn get_cached_image(&self, key: &EmbeddedImageCacheKey) -> Option<String> {
        self.cache_keys.borrow_mut().push(key.clone());
        self.cached_path.clone()
    }

    fn extract_image(
        &self,
        _request: EmbeddedImageExtractionRequest<'_>,
    ) -> Result<String, Self::Error> {
        Err("unexpected embedded image extraction")
    }
}

impl VideoImageCapability for FixtureCapability {
    fn extract_video_frame(
        &self,
        request: VideoFrameExtractionRequest<'_>,
    ) -> Result<String, Self::Error> {
        self.requests.borrow_mut().push(OwnedFrameRequest {
            item_path: request.item_path.to_owned(),
            container: request.container.map(ToOwned::to_owned),
            protocol: request.protocol,
            video_type: request.video_type,
            iso_type: request.iso_type,
            stream: request.stream.clone(),
            video_3d_format: request.video_3d_format,
            offset_ticks: request.offset_ticks,
        });
        if let Some(error) = self.error {
            Err(error)
        } else {
            Ok("path.jpg".to_owned())
        }
    }
}

#[test]
fn get_image_unsupported_input_returns_no_image_official_matrix() {
    for (item, default_index, streams) in [
        (
            EmbeddedImageItem {
                is_placeholder: true,
                ..movie()
            },
            None,
            Vec::new(),
        ),
        (movie(), None, Vec::new()),
        (movie(), Some(0), Vec::new()),
    ] {
        let capability = FixtureCapability::default();
        let response = VideoImageProvider::get_image(
            request(item, default_index, None),
            &streams,
            &capability,
        )
        .unwrap();
        assert!(!response.has_image);
        assert!(response.path.is_none());
        assert!(capability.requests.borrow().is_empty());
        assert!(capability.cache_keys.borrow().is_empty());
    }
}

#[test]
fn get_image_default_video_streams_returns_official_selection() {
    for (default_index, target_index) in [(1, 1), (5, 0)] {
        let streams = (0..=target_index).map(video_stream).collect::<Vec<_>>();
        let capability = FixtureCapability::default();
        let response = VideoImageProvider::get_image(
            request(movie(), Some(default_index), None),
            &streams,
            &capability,
        )
        .unwrap();

        assert!(response.has_image);
        assert_eq!(response.path.as_deref(), Some("path.jpg"));
        assert_eq!(response.format, Some(ImageFormat::Jpg));
        assert_eq!(response.protocol, MediaProtocol::File);
        assert_eq!(
            &capability.requests.borrow()[0].stream,
            streams
                .iter()
                .find(|stream| stream.index == target_index)
                .unwrap()
        );
    }
}

#[test]
fn get_image_time_span_selects_official_offset() {
    for (run_time_seconds, expected_seconds) in [(None, 10), (Some(500_i64), 50)] {
        let capability = FixtureCapability::default();
        VideoImageProvider::get_image(
            request(
                movie(),
                Some(0),
                run_time_seconds.map(|seconds| seconds * TICKS_PER_SECOND),
            ),
            &[video_stream(0)],
            &capability,
        )
        .unwrap();
        assert_eq!(
            capability.requests.borrow()[0].offset_ticks,
            expected_seconds * TICKS_PER_SECOND
        );
    }
}

#[test]
fn supports_primary_and_rejects_non_video_remote_shortcut_placeholder_and_incomplete() {
    assert_eq!(
        VideoImageProvider::get_supported_images(),
        [ImageType::Primary]
    );
    assert!(VideoImageProvider::supports(movie()));
    for item in [
        EmbeddedImageItem {
            kind: EmbeddedImageItemKind::AudioBook,
            ..movie()
        },
        EmbeddedImageItem {
            protocol: MediaProtocol::Http,
            ..movie()
        },
        EmbeddedImageItem {
            is_shortcut: true,
            ..movie()
        },
        EmbeddedImageItem {
            is_placeholder: true,
            ..movie()
        },
        EmbeddedImageItem {
            is_complete_media: false,
            ..movie()
        },
    ] {
        assert!(!VideoImageProvider::supports(item), "item: {item:?}");
    }
}

#[test]
fn embedded_image_stream_is_not_used_as_video_fallback() {
    let streams = [
        MediaStream {
            stream_type: MediaStreamType::EmbeddedImage,
            index: 0,
            ..MediaStream::default()
        },
        video_stream(1),
    ];
    let capability = FixtureCapability::default();
    VideoImageProvider::get_image(request(movie(), Some(0), None), &streams, &capability).unwrap();
    assert_eq!(capability.requests.borrow()[0].stream.index, 1);
}

#[test]
fn dvd_is_rejected_while_bluray_preserves_disc_source_fields() {
    let dvd = EmbeddedImageItem {
        video_type: VideoType::Dvd,
        iso_type: Some(IsoType::Dvd),
        ..movie()
    };
    let dvd_capability = FixtureCapability::default();
    let response = VideoImageProvider::get_image(
        request(dvd, Some(0), Some(500 * TICKS_PER_SECOND)),
        &[video_stream(0)],
        &dvd_capability,
    )
    .unwrap();
    assert!(!response.has_image);
    assert!(dvd_capability.requests.borrow().is_empty());

    let bluray = EmbeddedImageItem {
        path: "/disc/BDMV",
        container: Some("bluray"),
        video_type: VideoType::BluRay,
        iso_type: Some(IsoType::BluRay),
        ..movie()
    };
    let capability = FixtureCapability::default();
    VideoImageProvider::get_image(
        VideoImageRequest {
            item: bluray,
            default_video_stream_index: Some(2),
            run_time_ticks: Some(500 * TICKS_PER_SECOND),
            video_3d_format: Some(Video3DFormat::Mvc),
        },
        &[video_stream(2)],
        &capability,
    )
    .unwrap();
    let actual = &capability.requests.borrow()[0];
    assert_eq!(actual.item_path, "/disc/BDMV");
    assert_eq!(actual.container.as_deref(), Some("bluray"));
    assert_eq!(actual.video_type, VideoType::BluRay);
    assert_eq!(actual.iso_type, Some(IsoType::BluRay));
    assert_eq!(actual.video_3d_format, Some(Video3DFormat::Mvc));
    assert_eq!(actual.offset_ticks, 50 * TICKS_PER_SECOND);
}

#[test]
fn cache_key_is_stable_and_cache_hit_returns_jpg_file_response() {
    let capability = FixtureCapability {
        cached_path: Some("cache/frame.jpg".to_owned()),
        ..FixtureCapability::default()
    };
    let request = request(movie(), Some(0), Some(100 * TICKS_PER_SECOND));
    let stream = [video_stream(0)];
    let first = VideoImageProvider::get_image(request, &stream, &capability).unwrap();
    let second = VideoImageProvider::get_image(request, &stream, &capability).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.path.as_deref(), Some("cache/frame.jpg"));
    assert_eq!(first.protocol, MediaProtocol::File);
    assert_eq!(first.format, Some(ImageFormat::Jpg));
    assert_eq!(
        capability.cache_keys.borrow()[0],
        capability.cache_keys.borrow()[1]
    );
    assert!(!capability.cache_keys.borrow()[0].as_str().is_empty());
    assert!(capability.requests.borrow().is_empty());
}

#[test]
fn frame_extraction_error_is_propagated() {
    let capability = FixtureCapability {
        error: Some("fixture frame extraction failed"),
        ..FixtureCapability::default()
    };
    let result = VideoImageProvider::get_image(
        request(movie(), Some(0), None),
        &[video_stream(0)],
        &capability,
    );
    assert_eq!(result, Err("fixture frame extraction failed"));
    assert_eq!(capability.requests.borrow().len(), 1);
}

fn request(
    item: EmbeddedImageItem<'static>,
    default_video_stream_index: Option<i32>,
    run_time_ticks: Option<i64>,
) -> VideoImageRequest<'static> {
    VideoImageRequest {
        item,
        default_video_stream_index,
        run_time_ticks,
        video_3d_format: None,
    }
}

fn movie() -> EmbeddedImageItem<'static> {
    EmbeddedImageItem {
        kind: EmbeddedImageItemKind::Movie,
        path: "/media/movie.mkv",
        container: Some("mkv"),
        protocol: MediaProtocol::File,
        video_type: VideoType::VideoFile,
        iso_type: None,
        is_shortcut: false,
        is_placeholder: false,
        is_complete_media: true,
    }
}

fn video_stream(index: i32) -> MediaStream {
    MediaStream {
        stream_type: MediaStreamType::Video,
        index,
        ..MediaStream::default()
    }
}
