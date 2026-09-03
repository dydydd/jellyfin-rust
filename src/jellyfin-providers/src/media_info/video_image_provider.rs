use jellyfin_model::{
    ImageFormat, ImageType, MediaProtocol, MediaStream, MediaStreamType, VideoType,
};

use super::{
    EmbeddedImageCacheKey, EmbeddedImageCapability, EmbeddedImageItem, EmbeddedImageProvider,
    EmbeddedImageResponse, IsoType,
};

const TICKS_PER_SECOND: i64 = 10_000_000;

/// Three-dimensional layout passed through to frame extraction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Video3DFormat {
    HalfSideBySide,
    FullSideBySide,
    FullTopAndBottom,
    HalfTopAndBottom,
    Mvc,
}

/// Video item state needed by the screen-grabber provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoImageRequest<'a> {
    pub item: EmbeddedImageItem<'a>,
    pub default_video_stream_index: Option<i32>,
    pub run_time_ticks: Option<i64>,
    pub video_3d_format: Option<Video3DFormat>,
}

/// Exact frame extraction request passed to an encoder adapter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoFrameExtractionRequest<'a> {
    pub item_path: &'a str,
    pub container: Option<&'a str>,
    pub protocol: MediaProtocol,
    pub video_type: VideoType,
    pub iso_type: Option<IsoType>,
    pub stream: &'a MediaStream,
    pub video_3d_format: Option<Video3DFormat>,
    pub offset_ticks: i64,
}

/// Embedded-image cache capability extended with video frame extraction.
pub trait VideoImageCapability: EmbeddedImageCapability {
    /// Extracts a still frame from the selected video stream.
    ///
    /// # Errors
    ///
    /// Returns an implementation-defined error when frame extraction fails.
    fn extract_video_frame(
        &self,
        request: VideoFrameExtractionRequest<'_>,
    ) -> Result<String, Self::Error>;
}

/// Pure selection logic for Jellyfin's screen-grabber image provider.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VideoImageProvider;

impl VideoImageProvider {
    pub const NAME: &'static str = "Screen Grabber";
    pub const ORDER: i32 = 100;

    #[must_use]
    pub const fn get_supported_images() -> [ImageType; 1] {
        [ImageType::Primary]
    }

    #[must_use]
    pub const fn supports(item: EmbeddedImageItem<'_>) -> bool {
        EmbeddedImageProvider::supports(item)
    }

    /// Selects a video stream and requests a JPEG frame from the injected capability.
    ///
    /// # Errors
    ///
    /// Returns the injected capability's error when frame extraction fails.
    pub fn get_image<C: VideoImageCapability + ?Sized>(
        request: VideoImageRequest<'_>,
        media_streams: &[MediaStream],
        capability: &C,
    ) -> Result<EmbeddedImageResponse, C::Error> {
        if !Self::supports(request.item)
            || request.item.video_type == VideoType::Dvd
            || request.default_video_stream_index.is_none()
        {
            return Ok(EmbeddedImageResponse::no_image());
        }

        let default_index = request.default_video_stream_index.unwrap_or_default();
        let video_stream = media_streams
            .iter()
            .find(|stream| {
                stream.stream_type == MediaStreamType::Video && stream.index == default_index
            })
            .or_else(|| {
                media_streams
                    .iter()
                    .find(|stream| stream.stream_type == MediaStreamType::Video)
            });
        let Some(video_stream) = video_stream else {
            return Ok(EmbeddedImageResponse::no_image());
        };

        let offset_ticks = image_offset_ticks(request.item.video_type, request.run_time_ticks);
        let extraction_request = VideoFrameExtractionRequest {
            item_path: request.item.path,
            container: request.item.container,
            protocol: request.item.protocol,
            video_type: request.item.video_type,
            iso_type: request.item.iso_type,
            stream: video_stream,
            video_3d_format: request.video_3d_format,
            offset_ticks,
        };
        let cache_key = cache_key(extraction_request);
        let path = capability
            .get_cached_image(&cache_key)
            .map_or_else(|| capability.extract_video_frame(extraction_request), Ok)?;
        Ok(EmbeddedImageResponse {
            path: Some(path),
            protocol: MediaProtocol::File,
            format: Some(ImageFormat::Jpg),
            has_image: true,
            cache_key: Some(cache_key),
        })
    }
}

const fn image_offset_ticks(video_type: VideoType, run_time_ticks: Option<i64>) -> i64 {
    match run_time_ticks {
        Some(runtime) if !matches!(video_type, VideoType::Dvd) && runtime > 0 => runtime / 10,
        _ => 10 * TICKS_PER_SECOND,
    }
}

fn cache_key(request: VideoFrameExtractionRequest<'_>) -> EmbeddedImageCacheKey {
    let container = request.container.unwrap_or_default();
    let iso_type = match request.iso_type {
        None => -1,
        Some(IsoType::Dvd) => 0,
        Some(IsoType::BluRay) => 1,
    };
    let video_3d_format = request.video_3d_format.map_or(-1, |format| format as i32);
    EmbeddedImageCacheKey::from_value(format!(
        "v1-video|{}:{}|{}:{}|{}|{}|{}|{}|{}|{}",
        request.item_path.len(),
        request.item_path,
        container.len(),
        container,
        request.protocol as i32,
        request.video_type as i32,
        iso_type,
        request.stream.index,
        video_3d_format,
        request.offset_ticks,
    ))
}
