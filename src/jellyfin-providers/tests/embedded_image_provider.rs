use std::cell::RefCell;

use jellyfin_model::{
    ImageFormat, ImageType, MediaAttachment, MediaProtocol, MediaStream, MediaStreamType, VideoType,
};
use jellyfin_providers::media_info::{
    EmbeddedImageCacheKey, EmbeddedImageCapability, EmbeddedImageExtractionRequest,
    EmbeddedImageItem, EmbeddedImageItemKind, EmbeddedImageProvider, EmbeddedImageStream, IsoType,
};

#[derive(Clone, Debug, PartialEq)]
struct OwnedExtractionRequest {
    item_path: String,
    container: Option<String>,
    protocol: MediaProtocol,
    video_type: VideoType,
    iso_type: Option<IsoType>,
    stream: Option<MediaStream>,
    index: i32,
    format: ImageFormat,
    image_type: ImageType,
}

struct FixtureCapability {
    path_prefix: &'static str,
    cached_path: Option<String>,
    error: Option<&'static str>,
    cache_keys: RefCell<Vec<EmbeddedImageCacheKey>>,
    requests: RefCell<Vec<OwnedExtractionRequest>>,
}

impl Default for FixtureCapability {
    fn default() -> Self {
        Self {
            path_prefix: "path",
            cached_path: None,
            error: None,
            cache_keys: RefCell::new(Vec::new()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl EmbeddedImageCapability for FixtureCapability {
    type Error = &'static str;

    fn get_cached_image(&self, key: &EmbeddedImageCacheKey) -> Option<String> {
        self.cache_keys.borrow_mut().push(key.clone());
        self.cached_path.clone()
    }

    fn extract_image(
        &self,
        request: EmbeddedImageExtractionRequest<'_>,
    ) -> Result<String, Self::Error> {
        self.requests.borrow_mut().push(OwnedExtractionRequest {
            item_path: request.item_path.to_owned(),
            container: request.container.map(ToOwned::to_owned),
            protocol: request.protocol,
            video_type: request.video_type,
            iso_type: request.iso_type,
            stream: request.stream.cloned(),
            index: request.index,
            format: request.format,
            image_type: request.image_type,
        });
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(format!(
            "{}{}.{}",
            self.path_prefix,
            request.index,
            format_name(request.format)
        ))
    }
}

#[test]
fn get_supported_images_any_base_item_returns_official_matrix() {
    for (kind, expected) in [
        (EmbeddedImageItemKind::AudioBook, vec![]),
        (EmbeddedImageItemKind::BoxSet, vec![]),
        (EmbeddedImageItemKind::Series, vec![]),
        (EmbeddedImageItemKind::Season, vec![]),
        (EmbeddedImageItemKind::Episode, vec![ImageType::Primary]),
        (
            EmbeddedImageItemKind::Movie,
            vec![ImageType::Logo, ImageType::Backdrop, ImageType::Primary],
        ),
    ] {
        let mut actual = EmbeddedImageProvider::get_supported_images(kind);
        actual.sort_by_key(|image_type| *image_type as i32);
        let mut expected = expected;
        expected.sort_by_key(|image_type| *image_type as i32);
        assert_eq!(actual, expected, "kind: {kind:?}");
    }
}

#[test]
fn get_image_no_streams_returns_no_image() {
    let capability = FixtureCapability::default();
    let response =
        EmbeddedImageProvider::get_image(movie(), ImageType::Primary, &[], &[], &capability)
            .unwrap();
    assert!(!response.has_image);
    assert!(response.path.is_none());
    assert!(capability.requests.borrow().is_empty());
}

#[test]
fn get_image_attachment_returns_official_selection_and_format_matrix() {
    for (file_name, mime_type, target_index, image_type, expected_format) in [
        ("chapter", None, 1, ImageType::Chapter, None),
        ("unmatched", None, 1, ImageType::Primary, None),
        (
            "clearlogo.png",
            None,
            1,
            ImageType::Logo,
            Some(ImageFormat::Png),
        ),
        (
            "backdrop",
            Some("image/bmp"),
            2,
            ImageType::Backdrop,
            Some(ImageFormat::Bmp),
        ),
        (
            "poster",
            None,
            3,
            ImageType::Primary,
            Some(ImageFormat::Jpg),
        ),
    ] {
        let attachments = (1..=target_index)
            .map(|index| MediaAttachment {
                index,
                file_name: Some(if index == target_index {
                    file_name.to_owned()
                } else {
                    "unmatched".to_owned()
                }),
                mime_type: mime_type.map(ToOwned::to_owned),
                ..MediaAttachment::default()
            })
            .collect::<Vec<_>>();
        let capability = FixtureCapability::default();
        let response =
            EmbeddedImageProvider::get_image(movie(), image_type, &attachments, &[], &capability)
                .unwrap();

        assert_eq!(response.has_image, expected_format.is_some());
        assert_eq!(response.format, expected_format);
        if let Some(format) = expected_format {
            assert_eq!(
                response.path.as_deref(),
                Some(format!("path{target_index}.{}", format_name(format)).as_str())
            );
            let request = &capability.requests.borrow()[0];
            assert_eq!(request.index, target_index);
            assert_eq!(request.format, format);
            assert!(request.stream.is_none());
        } else {
            assert!(capability.requests.borrow().is_empty());
        }
    }
}

#[test]
fn get_image_embedded_returns_official_selection_and_format_matrix() {
    for (label, codec, target_index, image_type, expected_format) in [
        ("chapter", None, 1, ImageType::Chapter, None),
        ("", None, 1, ImageType::Backdrop, None),
        ("", None, 1, ImageType::Primary, Some(ImageFormat::Jpg)),
        (
            "backdrop",
            None,
            2,
            ImageType::Backdrop,
            Some(ImageFormat::Jpg),
        ),
        ("cover", None, 2, ImageType::Primary, Some(ImageFormat::Jpg)),
        (
            "",
            Some("bmp"),
            1,
            ImageType::Primary,
            Some(ImageFormat::Bmp),
        ),
        (
            "",
            Some("gif"),
            1,
            ImageType::Primary,
            Some(ImageFormat::Gif),
        ),
        (
            "",
            Some("mjpeg"),
            1,
            ImageType::Primary,
            Some(ImageFormat::Jpg),
        ),
        (
            "",
            Some("png"),
            1,
            ImageType::Primary,
            Some(ImageFormat::Png),
        ),
        (
            "",
            Some("webp"),
            1,
            ImageType::Primary,
            Some(ImageFormat::Webp),
        ),
    ] {
        let streams = (1..=target_index)
            .map(|index| EmbeddedImageStream {
                stream: MediaStream {
                    stream_type: MediaStreamType::EmbeddedImage,
                    index,
                    codec: codec.map(ToOwned::to_owned),
                    ..MediaStream::default()
                },
                comment: (index == target_index && !label.is_empty())
                    .then(|| label.to_owned())
                    .or_else(|| (index != target_index).then(|| "unmatched".to_owned())),
            })
            .collect::<Vec<_>>();
        let capability = FixtureCapability::default();
        let response =
            EmbeddedImageProvider::get_image(movie(), image_type, &[], &streams, &capability)
                .unwrap();

        assert_eq!(response.has_image, expected_format.is_some());
        assert_eq!(response.format, expected_format);
        if let Some(format) = expected_format {
            let request = &capability.requests.borrow()[0];
            assert_eq!(request.index, target_index);
            assert_eq!(request.format, format);
            assert_eq!(
                request.stream.as_ref(),
                streams
                    .iter()
                    .find(|stream| stream.stream.index == target_index)
                    .map(|stream| &stream.stream)
            );
        } else {
            assert!(capability.requests.borrow().is_empty());
        }
    }
}

#[test]
fn attachments_take_precedence_over_matching_embedded_streams() {
    let attachments = [MediaAttachment {
        index: 7,
        file_name: Some("poster.png".to_owned()),
        mime_type: None,
        ..MediaAttachment::default()
    }];
    let streams = [EmbeddedImageStream {
        stream: MediaStream {
            stream_type: MediaStreamType::EmbeddedImage,
            index: 9,
            ..MediaStream::default()
        },
        comment: Some("cover".to_owned()),
    }];
    let capability = FixtureCapability::default();
    EmbeddedImageProvider::get_image(
        movie(),
        ImageType::Primary,
        &attachments,
        &streams,
        &capability,
    )
    .unwrap();
    let request = &capability.requests.borrow()[0];
    assert_eq!(request.index, 7);
    assert!(request.stream.is_none());
}

#[test]
fn nonempty_unknown_attachment_mime_type_uses_jpg_fallback() {
    let attachments = [MediaAttachment {
        index: 1,
        file_name: Some("poster.png".to_owned()),
        mime_type: Some("application/x-unknown-embedded-image".to_owned()),
        ..MediaAttachment::default()
    }];
    let capability = FixtureCapability::default();
    let response = EmbeddedImageProvider::get_image(
        movie(),
        ImageType::Primary,
        &attachments,
        &[],
        &capability,
    )
    .unwrap();
    assert_eq!(response.format, Some(ImageFormat::Jpg));
}

#[test]
fn supports_rejects_non_video_shortcut_remote_placeholder_and_incomplete_items() {
    let baseline = movie();
    assert!(EmbeddedImageProvider::supports(baseline));
    for item in [
        EmbeddedImageItem {
            kind: EmbeddedImageItemKind::AudioBook,
            ..baseline
        },
        EmbeddedImageItem {
            is_shortcut: true,
            ..baseline
        },
        EmbeddedImageItem {
            protocol: MediaProtocol::Http,
            ..baseline
        },
        EmbeddedImageItem {
            is_placeholder: true,
            ..baseline
        },
        EmbeddedImageItem {
            is_complete_media: false,
            ..baseline
        },
    ] {
        assert!(!EmbeddedImageProvider::supports(item), "item: {item:?}");
    }
}

#[test]
fn get_image_rejects_protocol_placeholder_and_dvd_before_extraction() {
    let attachment = [MediaAttachment {
        index: 1,
        file_name: Some("poster.png".to_owned()),
        mime_type: None,
        ..MediaAttachment::default()
    }];
    for item in [
        EmbeddedImageItem {
            protocol: MediaProtocol::Http,
            ..movie()
        },
        EmbeddedImageItem {
            is_placeholder: true,
            ..movie()
        },
        EmbeddedImageItem {
            video_type: VideoType::Dvd,
            ..movie()
        },
    ] {
        let capability = FixtureCapability::default();
        let response = EmbeddedImageProvider::get_image(
            item,
            ImageType::Primary,
            &attachment,
            &[],
            &capability,
        )
        .unwrap();
        assert!(!response.has_image);
        assert!(capability.requests.borrow().is_empty());
    }
}

#[test]
fn cache_key_is_stable_and_cache_hit_returns_file_response_without_extraction() {
    let attachment = [MediaAttachment {
        index: 3,
        file_name: Some("poster.webp".to_owned()),
        mime_type: None,
        ..MediaAttachment::default()
    }];
    let capability = FixtureCapability {
        cached_path: Some("cache/poster.webp".to_owned()),
        ..FixtureCapability::default()
    };
    let first = EmbeddedImageProvider::get_image(
        movie(),
        ImageType::Primary,
        &attachment,
        &[],
        &capability,
    )
    .unwrap();
    let second = EmbeddedImageProvider::get_image(
        movie(),
        ImageType::Primary,
        &attachment,
        &[],
        &capability,
    )
    .unwrap();

    assert_eq!(first, second);
    assert!(first.has_image);
    assert_eq!(first.path.as_deref(), Some("cache/poster.webp"));
    assert_eq!(first.protocol, MediaProtocol::File);
    assert_eq!(first.format, Some(ImageFormat::Webp));
    assert_eq!(
        capability.cache_keys.borrow()[0],
        capability.cache_keys.borrow()[1]
    );
    assert!(!capability.cache_keys.borrow()[0].as_str().is_empty());
    assert!(capability.requests.borrow().is_empty());
}

#[test]
fn extraction_error_is_propagated_without_success_response() {
    let attachment = [MediaAttachment {
        index: 1,
        file_name: Some("poster.jpg".to_owned()),
        mime_type: None,
        ..MediaAttachment::default()
    }];
    let capability = FixtureCapability {
        error: Some("fixture extraction failed"),
        ..FixtureCapability::default()
    };
    let result = EmbeddedImageProvider::get_image(
        movie(),
        ImageType::Primary,
        &attachment,
        &[],
        &capability,
    );
    assert_eq!(result, Err("fixture extraction failed"));
    assert_eq!(capability.requests.borrow().len(), 1);
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

const fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Bmp => "Bmp",
        ImageFormat::Gif => "Gif",
        ImageFormat::Jpg => "Jpg",
        ImageFormat::Png => "Png",
        ImageFormat::Webp => "Webp",
        ImageFormat::Svg => "Svg",
    }
}
