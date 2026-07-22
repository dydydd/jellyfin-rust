use jellyfin_model::{
    ImageFormat, ImageType, MediaProtocol, MediaStream, MediaStreamType, MimeTypes, VideoType,
};

use super::IsoType;

const PRIMARY_IMAGE_NAMES: &[&str] = &["poster", "folder", "cover", "default", "movie", "show"];
const BACKDROP_IMAGE_NAMES: &[&str] = &["backdrop", "background", "art"];
const LOGO_IMAGE_NAMES: &[&str] = &["logo"];

/// Item category used by the embedded-image support matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedImageItemKind {
    AudioBook,
    BoxSet,
    Series,
    Season,
    Episode,
    Movie,
    Video,
}

impl EmbeddedImageItemKind {
    const fn is_video(self) -> bool {
        matches!(self, Self::Episode | Self::Movie | Self::Video)
    }
}

/// Item fields used to select and extract an embedded image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddedImageItem<'a> {
    pub kind: EmbeddedImageItemKind,
    pub path: &'a str,
    pub container: Option<&'a str>,
    pub protocol: MediaProtocol,
    pub video_type: VideoType,
    pub iso_type: Option<IsoType>,
    pub is_shortcut: bool,
    pub is_placeholder: bool,
    pub is_complete_media: bool,
}

/// Stored attachment metadata considered before embedded image streams.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaAttachment {
    pub index: i32,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
}

/// Model stream plus the ffprobe comment used as its image label.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbeddedImageStream {
    pub stream: MediaStream,
    pub comment: Option<String>,
}

/// Stable identifier for an extracted image cache entry.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EmbeddedImageCacheKey(String);

impl EmbeddedImageCacheKey {
    pub(crate) fn from_value(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Selected extraction input passed to an image encoder adapter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmbeddedImageExtractionRequest<'a> {
    pub item_path: &'a str,
    pub container: Option<&'a str>,
    pub protocol: MediaProtocol,
    pub video_type: VideoType,
    pub iso_type: Option<IsoType>,
    pub stream: Option<&'a MediaStream>,
    pub index: i32,
    pub format: ImageFormat,
    pub image_type: ImageType,
}

/// Boundary for cache lookup and the actual image extraction process.
pub trait EmbeddedImageCapability {
    type Error;

    fn get_cached_image(&self, _key: &EmbeddedImageCacheKey) -> Option<String> {
        None
    }

    /// Extracts the selected attachment or embedded stream.
    ///
    /// # Errors
    ///
    /// Returns an implementation-defined error when extraction fails.
    fn extract_image(
        &self,
        request: EmbeddedImageExtractionRequest<'_>,
    ) -> Result<String, Self::Error>;
}

/// Dynamic image result matching Jellyfin's provider response fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedImageResponse {
    pub path: Option<String>,
    pub protocol: MediaProtocol,
    pub format: Option<ImageFormat>,
    pub has_image: bool,
    pub cache_key: Option<EmbeddedImageCacheKey>,
}

impl EmbeddedImageResponse {
    pub(crate) const fn no_image() -> Self {
        Self {
            path: None,
            protocol: MediaProtocol::File,
            format: None,
            has_image: false,
            cache_key: None,
        }
    }
}

/// Pure selection logic for Jellyfin's embedded image extractor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbeddedImageProvider;

impl EmbeddedImageProvider {
    pub const NAME: &'static str = "Embedded Image Extractor";
    pub const ORDER: i32 = 99;

    #[must_use]
    pub fn get_supported_images(item_kind: EmbeddedImageItemKind) -> Vec<ImageType> {
        match item_kind {
            EmbeddedImageItemKind::Episode => vec![ImageType::Primary],
            EmbeddedImageItemKind::Movie | EmbeddedImageItemKind::Video => {
                vec![ImageType::Primary, ImageType::Backdrop, ImageType::Logo]
            }
            EmbeddedImageItemKind::AudioBook
            | EmbeddedImageItemKind::BoxSet
            | EmbeddedImageItemKind::Series
            | EmbeddedImageItemKind::Season => Vec::new(),
        }
    }

    #[must_use]
    pub const fn supports(item: EmbeddedImageItem<'_>) -> bool {
        item.kind.is_video()
            && !item.is_shortcut
            && matches!(item.protocol, MediaProtocol::File)
            && !item.is_placeholder
            && item.is_complete_media
    }

    /// Selects and extracts an attachment or embedded image stream.
    ///
    /// # Errors
    ///
    /// Returns the injected capability's error when extraction fails.
    pub fn get_image<C: EmbeddedImageCapability + ?Sized>(
        item: EmbeddedImageItem<'_>,
        image_type: ImageType,
        attachments: &[MediaAttachment],
        image_streams: &[EmbeddedImageStream],
        capability: &C,
    ) -> Result<EmbeddedImageResponse, C::Error> {
        if !Self::supports(item) || matches!(item.video_type, VideoType::Dvd) {
            return Ok(EmbeddedImageResponse::no_image());
        }
        let image_names = image_names(image_type);
        if image_names.is_empty() {
            return Ok(EmbeddedImageResponse::no_image());
        }

        if let Some(attachment) = attachments
            .iter()
            .find(|attachment| attachment_matches(attachment, image_names))
        {
            let format = attachment_format(attachment);
            return extract(
                item,
                image_type,
                None,
                attachment.index,
                format,
                "attachment",
                capability,
            );
        }

        let streams = image_streams
            .iter()
            .filter(|stream| stream.stream.stream_type == MediaStreamType::EmbeddedImage)
            .collect::<Vec<_>>();
        let selected = streams
            .iter()
            .copied()
            .find(|stream| stream_matches(stream, image_names))
            .or_else(|| {
                (image_type == ImageType::Primary)
                    .then(|| streams.first().copied())
                    .flatten()
            });
        let Some(selected) = selected else {
            return Ok(EmbeddedImageResponse::no_image());
        };
        let format = stream_format(selected.stream.codec.as_deref());
        extract(
            item,
            image_type,
            Some(&selected.stream),
            selected.stream.index,
            format,
            "stream",
            capability,
        )
    }
}

const fn image_names(image_type: ImageType) -> &'static [&'static str] {
    match image_type {
        ImageType::Primary => PRIMARY_IMAGE_NAMES,
        ImageType::Backdrop => BACKDROP_IMAGE_NAMES,
        ImageType::Logo => LOGO_IMAGE_NAMES,
        ImageType::Art
        | ImageType::Banner
        | ImageType::Thumb
        | ImageType::Disc
        | ImageType::Box
        | ImageType::Screenshot
        | ImageType::Menu
        | ImageType::Chapter
        | ImageType::BoxRear
        | ImageType::Profile => &[],
    }
}

fn attachment_matches(attachment: &MediaAttachment, image_names: &[&str]) -> bool {
    attachment
        .file_name
        .as_deref()
        .is_some_and(|file_name| contains_any_ignore_ascii_case(file_name, image_names))
}

fn stream_matches(stream: &EmbeddedImageStream, image_names: &[&str]) -> bool {
    stream
        .comment
        .as_deref()
        .is_some_and(|comment| contains_any_ignore_ascii_case(comment, image_names))
}

fn contains_any_ignore_ascii_case(value: &str, candidates: &[&str]) -> bool {
    let value = value.to_ascii_lowercase();
    candidates
        .iter()
        .any(|candidate| value.contains(&candidate.to_ascii_lowercase()))
}

fn attachment_format(attachment: &MediaAttachment) -> ImageFormat {
    let extension = match attachment
        .mime_type
        .as_deref()
        .filter(|mime_type| !mime_type.is_empty())
    {
        Some(mime_type) => MimeTypes::to_extension(mime_type).ok().flatten(),
        None => attachment
            .file_name
            .as_deref()
            .and_then(file_extension)
            .map(ToOwned::to_owned),
    };
    format_from_extension(extension.as_deref())
}

fn file_extension(path: &str) -> Option<&str> {
    let file_name = path.rsplit(['/', '\\']).next()?;
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| &file_name[file_name.len() - extension.len() - 1..])
}

fn format_from_extension(extension: Option<&str>) -> ImageFormat {
    match extension {
        Some(".bmp") => ImageFormat::Bmp,
        Some(".gif") => ImageFormat::Gif,
        Some(".png") => ImageFormat::Png,
        Some(".webp") => ImageFormat::Webp,
        _ => ImageFormat::Jpg,
    }
}

fn stream_format(codec: Option<&str>) -> ImageFormat {
    match codec {
        Some("bmp") => ImageFormat::Bmp,
        Some("gif") => ImageFormat::Gif,
        Some("png") => ImageFormat::Png,
        Some("webp") => ImageFormat::Webp,
        _ => ImageFormat::Jpg,
    }
}

fn extract<C: EmbeddedImageCapability + ?Sized>(
    item: EmbeddedImageItem<'_>,
    image_type: ImageType,
    stream: Option<&MediaStream>,
    index: i32,
    format: ImageFormat,
    source: &str,
    capability: &C,
) -> Result<EmbeddedImageResponse, C::Error> {
    let request = EmbeddedImageExtractionRequest {
        item_path: item.path,
        container: item.container,
        protocol: item.protocol,
        video_type: item.video_type,
        iso_type: item.iso_type,
        stream,
        index,
        format,
        image_type,
    };
    let cache_key = cache_key(request, source);
    let path = capability
        .get_cached_image(&cache_key)
        .map_or_else(|| capability.extract_image(request), Ok)?;
    Ok(EmbeddedImageResponse {
        path: Some(path),
        protocol: MediaProtocol::File,
        format: Some(format),
        has_image: true,
        cache_key: Some(cache_key),
    })
}

fn cache_key(request: EmbeddedImageExtractionRequest<'_>, source: &str) -> EmbeddedImageCacheKey {
    let container = request.container.unwrap_or_default();
    let iso_type = match request.iso_type {
        None => -1,
        Some(IsoType::Dvd) => 0,
        Some(IsoType::BluRay) => 1,
    };
    EmbeddedImageCacheKey(format!(
        "v1|{}:{}|{}:{}|{}|{}|{}|{}|{}|{}|{}",
        request.item_path.len(),
        request.item_path,
        container.len(),
        container,
        request.protocol as i32,
        request.video_type as i32,
        iso_type,
        request.image_type as i32,
        source,
        request.index,
        request.format as i32,
    ))
}
