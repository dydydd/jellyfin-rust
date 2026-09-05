//! Image decoding, transformation, and disk caching for Jellyfin.

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufReader, BufWriter, Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};

use image::{
    DynamicImage, GenericImageView, ImageEncoder, ImageFormat as DecoderFormat, ImageReader, Rgba,
    RgbaImage, imageops::FilterType,
};
use jellyfin_model::ImageFormat;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard, Semaphore};

const CACHE_VERSION: u8 = 2;
const DEFAULT_QUALITY: u8 = 100;

/// A source image and the metadata needed to safely key processed-image caches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageSource {
    pub path: PathBuf,
    pub date_modified: SystemTime,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl ImageSource {
    pub fn new(path: impl Into<PathBuf>, date_modified: SystemTime) -> Self {
        Self {
            path: path.into(),
            date_modified,
            width: None,
            height: None,
        }
    }

    #[must_use]
    pub const fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }
}

/// Transformations requested for a source image.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageProcessingRequest {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub fill_width: Option<u32>,
    pub fill_height: Option<u32>,
    pub quality: u8,
    /// A convenience override for callers that require exactly one format.
    pub format: Option<ImageFormat>,
    /// Client-supported formats, in preference order.
    pub supported_formats: Vec<ImageFormat>,
    pub blur: Option<u32>,
    pub percent_played: Option<f64>,
    pub unplayed_count: Option<i32>,
    pub background_color: Option<String>,
    pub foreground_layer: Option<String>,
}

impl Default for ImageProcessingRequest {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            fill_width: None,
            fill_height: None,
            quality: DEFAULT_QUALITY,
            format: None,
            supported_formats: vec![
                ImageFormat::Webp,
                ImageFormat::Jpg,
                ImageFormat::Png,
                ImageFormat::Gif,
                ImageFormat::Bmp,
            ],
            blur: None,
            percent_played: None,
            unplayed_count: None,
            background_color: None,
            foreground_layer: None,
        }
    }
}

/// A file ready to be returned by the image API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessedImage {
    pub path: PathBuf,
    pub mime_type: &'static str,
    pub date_modified: SystemTime,
}

/// Typed failures produced while validating or processing an image.
#[derive(Debug, Error)]
pub enum ImageProcessingError {
    #[error("image encoding concurrency limit must be at least one")]
    InvalidConcurrencyLimit,
    #[error("collage requires at least one input and positive dimensions")]
    InvalidCollageOptions,
    #[error("image processor concurrency limiter was closed")]
    SemaphoreClosed,
    #[error("image quality must be between 1 and 100, got {0}")]
    InvalidQuality(u8),
    #[error("percent played must be finite")]
    InvalidPercentPlayed,
    #[error("image output format is not supported: {0:?}")]
    UnsupportedOutputFormat(ImageFormat),
    #[error("none of the requested image output formats are supported")]
    NoSupportedOutputFormat,
    #[error("invalid background color: {0}")]
    InvalidBackgroundColor(String),
    #[error("source image format could not be determined: {0}")]
    UnknownSourceFormat(PathBuf),
    #[error("could not access image file {path}: {source}")]
    FileAccess {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not decode image file {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("could not encode image cache file {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("image processing task failed: {0}")]
    ProcessingTask(#[from] tokio::task::JoinError),
}

/// Typed failures produced while inspecting an uploaded image's dimensions.
#[derive(Debug, Error)]
pub enum ImageInspectionError {
    #[error("could not access image file {path}: {source}")]
    FileAccess {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("source image format could not be determined: {0}")]
    UnknownFormat(PathBuf),
    #[error("could not inspect image file {path}: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("image inspection task failed: {0}")]
    InspectionTask(#[from] tokio::task::JoinError),
}

/// Typed failures produced while decoding an image for `BlurHash` generation.
#[derive(Debug, Error)]
pub enum BlurHashError {
    #[error("could not access image file {path}: {source}")]
    FileAccess {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("source image format could not be determined: {0}")]
    UnknownFormat(PathBuf),
    #[error("could not decode image file {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("could not encode image BlurHash: {0}")]
    Encode(#[from] blurhash::Error),
    #[error("BlurHash generation task failed: {0}")]
    GenerationTask(#[from] tokio::task::JoinError),
}

/// Decodes a local raster image and generates Jellyfin-compatible `BlurHash` metadata.
///
/// The component counts follow Jellyfin's roughly-sixteen-square-tiles formula,
/// while the pixel buffer is capped at 128x128 for predictable CPU cost.
///
/// # Errors
///
/// Returns [`BlurHashError`] for inaccessible, unsupported, malformed, or failed images.
pub async fn generate_blur_hash(
    path: impl AsRef<Path>,
) -> Result<(u32, u32, String), BlurHashError> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || generate_blur_hash_blocking(&path)).await?
}

fn generate_blur_hash_blocking(path: &Path) -> Result<(u32, u32, String), BlurHashError> {
    let file = fs::File::open(path).map_err(|source| BlurHashError::FileAccess {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|source| BlurHashError::FileAccess {
            path: path.to_path_buf(),
            source,
        })?;
    if reader.format().is_none() {
        return Err(BlurHashError::UnknownFormat(path.to_path_buf()));
    }
    let image = reader.decode().map_err(|source| BlurHashError::Decode {
        path: path.to_path_buf(),
        source,
    })?;
    let (width, height) = image.dimensions();
    let (components_x, components_y) = blur_hash_components(width, height);
    let pixels = image.thumbnail(128, 128).to_rgba8();
    let hash = blurhash::encode(
        components_x,
        components_y,
        pixels.width(),
        pixels.height(),
        pixels.as_raw(),
    )?;
    Ok((width, height, hash))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn blur_hash_components(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let x_float = (16.0 * width / height).sqrt();
    let y_float = x_float * height / width;
    (
        (x_float as u32).saturating_add(1).clamp(1, 9),
        (y_float as u32).saturating_add(1).clamp(1, 9),
    )
}

/// Reads an image's dimensions from its encoded data without decoding its pixel buffer.
///
/// The file contents, rather than its extension, determine the decoder. Upload callers can treat
/// an error as a best-effort probe failure and retain the original file for Jellyfin compatibility.
///
/// # Errors
///
/// Returns [`ImageInspectionError`] when the file cannot be accessed, its format is unknown, its
/// header is invalid or unsupported, or the blocking inspection task fails.
pub async fn inspect_dimensions(
    path: impl AsRef<Path>,
) -> Result<(u32, u32), ImageInspectionError> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || inspect_dimensions_blocking(&path)).await?
}

fn inspect_dimensions_blocking(path: &Path) -> Result<(u32, u32), ImageInspectionError> {
    let file = fs::File::open(path).map_err(|source| ImageInspectionError::FileAccess {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|source| ImageInspectionError::FileAccess {
            path: path.to_path_buf(),
            source,
        })?;
    if reader.format().is_none() {
        return Err(ImageInspectionError::UnknownFormat(path.to_path_buf()));
    }
    reader
        .into_dimensions()
        .map_err(|source| ImageInspectionError::Inspect {
            path: path.to_path_buf(),
            source,
        })
}

/// Processes images into a stable on-disk cache while bounding memory-heavy work.
#[derive(Clone, Debug)]
pub struct ImageProcessor {
    cache_directory: Arc<PathBuf>,
    encoding_limit: Arc<Semaphore>,
    encoding_locks: Arc<StdMutex<HashMap<PathBuf, Weak<Mutex<()>>>>>,
    #[cfg(test)]
    test_instrumentation: Arc<TestEncodingInstrumentation>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct TestEncodingInstrumentation {
    arrivals: StdMutex<Option<Arc<tokio::sync::Barrier>>>,
    attempts: std::sync::atomic::AtomicUsize,
    delay: StdMutex<Option<std::time::Duration>>,
}

struct EncodingLockEntry {
    cache_path: PathBuf,
    lock: Arc<Mutex<()>>,
    registry: Arc<StdMutex<HashMap<PathBuf, Weak<Mutex<()>>>>>,
}

impl EncodingLockEntry {
    async fn acquire(self) -> EncodingLockGuard {
        let guard = Arc::clone(&self.lock).lock_owned().await;
        EncodingLockGuard {
            guard: Some(guard),
            _entry: self,
        }
    }
}

impl Drop for EncodingLockEntry {
    fn drop(&mut self) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let is_current_entry = registry
            .get(&self.cache_path)
            .is_some_and(|entry| entry.ptr_eq(&Arc::downgrade(&self.lock)));
        if is_current_entry && Arc::strong_count(&self.lock) == 1 {
            registry.remove(&self.cache_path);
        }
    }
}

struct EncodingLockGuard {
    // Drop the mutex guard before the entry checks whether it is the final user.
    guard: Option<OwnedMutexGuard<()>>,
    _entry: EncodingLockEntry,
}

impl Drop for EncodingLockGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
    }
}

/// Inputs and layout for Jellyfin-style dynamic image collages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageCollageOptions {
    pub input_paths: Vec<PathBuf>,
    pub output_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub thumb_layout: bool,
}

impl ImageProcessor {
    /// Creates an image processor with a compile-time positive concurrency limit.
    #[must_use]
    pub fn with_concurrency<const LIMIT: usize>(cache_directory: impl Into<PathBuf>) -> Self {
        const {
            assert!(
                LIMIT > 0,
                "image processor concurrency limit must be positive"
            );
        };
        Self {
            cache_directory: Arc::new(cache_directory.into()),
            encoding_limit: Arc::new(Semaphore::new(LIMIT)),
            encoding_locks: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            test_instrumentation: Arc::new(TestEncodingInstrumentation::default()),
        }
    }

    /// Creates an image processor using `cache_directory` for derived files.
    ///
    /// # Errors
    ///
    /// Returns [`ImageProcessingError::InvalidConcurrencyLimit`] when the limit is zero.
    pub fn new(
        cache_directory: impl Into<PathBuf>,
        max_concurrent_encodes: usize,
    ) -> Result<Self, ImageProcessingError> {
        if max_concurrent_encodes == 0 {
            return Err(ImageProcessingError::InvalidConcurrencyLimit);
        }

        Ok(Self {
            cache_directory: Arc::new(cache_directory.into()),
            encoding_limit: Arc::new(Semaphore::new(max_concurrent_encodes)),
            encoding_locks: Arc::new(StdMutex::new(HashMap::new())),
            #[cfg(test)]
            test_instrumentation: Arc::new(TestEncodingInstrumentation::default()),
        })
    }

    #[must_use]
    pub fn cache_directory(&self) -> &Path {
        self.cache_directory.as_path()
    }

    /// Returns the source unchanged when possible, or creates/reuses a transformed cache file.
    ///
    /// # Errors
    ///
    /// Returns a typed [`ImageProcessingError`] when validation, file access, decoding, or
    /// encoding fails.
    pub async fn process(
        &self,
        source: ImageSource,
        request: ImageProcessingRequest,
    ) -> Result<ProcessedImage, ImageProcessingError> {
        let normalized = NormalizedRequest::new(request)?;
        let source_format = format_from_path(&source.path);

        ensure_source_exists(&source.path).await?;
        if normalized.can_return_original(&source, source_format)
            && let Some(format) = source_format
        {
            return Ok(ProcessedImage {
                path: source.path,
                mime_type: format.mime_type(),
                date_modified: source.date_modified,
            });
        }

        let output_format = normalized.output_format(source_format)?;
        let original = ProcessedImage {
            path: source.path.clone(),
            mime_type: source_format.map_or("application/octet-stream", ImageFormat::mime_type),
            date_modified: source.date_modified,
        };
        let cache_path = cache_path(
            self.cache_directory.as_path(),
            &source,
            &normalized,
            output_format,
        );
        if let Some(result) = cached_result(&cache_path, output_format).await? {
            return Ok(result);
        }

        #[cfg(test)]
        let arrivals = self
            .test_instrumentation
            .arrivals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        #[cfg(test)]
        if let Some(arrivals) = arrivals {
            arrivals.wait().await;
        }

        // Key by the final derived path so identical cold-cache requests share one decode and
        // encode. Acquire this before the global limit so followers do not consume scarce permits.
        let encoding_lock = self.encoding_lock(cache_path.clone()).acquire().await;
        if let Some(result) = cached_result(&cache_path, output_format).await? {
            return Ok(result);
        }

        let permit = Arc::clone(&self.encoding_limit)
            .acquire_owned()
            .await
            .map_err(|_| ImageProcessingError::SemaphoreClosed)?;

        let source_path = source.path;
        #[cfg(test)]
        let test_instrumentation = Arc::clone(&self.test_instrumentation);
        let requested_cache_path = cache_path.clone();
        let encoded = tokio::task::spawn_blocking(move || {
            let _encoding_lock = encoding_lock;
            let _permit = permit;
            #[cfg(test)]
            {
                use std::sync::atomic::Ordering;

                test_instrumentation.attempts.fetch_add(1, Ordering::SeqCst);
                if let Some(delay) = *test_instrumentation
                    .delay
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                {
                    std::thread::sleep(delay);
                }
            }
            process_to_cache(&source_path, &cache_path, &normalized, output_format)?;
            Ok::<_, ImageProcessingError>(cache_path)
        })
        .await?;
        let cache_path = match encoded {
            Ok(cache_path) => cache_path,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    source_path = %original.path.display(),
                    cache_path = %requested_cache_path.display(),
                    "error converting image; returning the original file"
                );
                return Ok(original);
            }
        };

        cached_result(&cache_path, output_format)
            .await?
            .ok_or_else(|| ImageProcessingError::FileAccess {
                path: cache_path,
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "encoded cache file disappeared",
                ),
            })
    }

    fn encoding_lock(&self, cache_path: PathBuf) -> EncodingLockEntry {
        let lock = {
            let mut registry = self
                .encoding_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(lock) = registry.get(&cache_path).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                registry.insert(cache_path.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        EncodingLockEntry {
            cache_path,
            lock,
            registry: Arc::clone(&self.encoding_locks),
        }
    }
}

/// Creates a grid or thumb collage from the supplied raster images.
///
/// The output is always JPEG, matching Jellyfin's generated collection images.
///
/// # Errors
///
/// Returns a typed [`ImageProcessingError`] when an input cannot be decoded or
/// the output cannot be written.
pub async fn create_collage(options: ImageCollageOptions) -> Result<PathBuf, ImageProcessingError> {
    if options.input_paths.is_empty() || options.width == 0 || options.height == 0 {
        return Err(ImageProcessingError::InvalidCollageOptions);
    }
    tokio::task::spawn_blocking(move || {
        let canvas = build_collage(&options)?;
        let output_path = options.output_path;
        if let Err(source) = canvas.save_with_format(&output_path, image::ImageFormat::Jpeg) {
            return Err(ImageProcessingError::Encode {
                path: output_path,
                source,
            });
        }
        Ok(output_path)
    })
    .await?
}

fn build_collage(options: &ImageCollageOptions) -> Result<DynamicImage, ImageProcessingError> {
    let inputs = options
        .input_paths
        .iter()
        .cloned()
        .map(|path| {
            let reader = match ImageReader::open(&path) {
                Ok(reader) => reader,
                Err(source) => {
                    return Err(ImageProcessingError::FileAccess { path, source });
                }
            };
            let reader = match reader.with_guessed_format() {
                Ok(reader) => reader,
                Err(source) => {
                    return Err(ImageProcessingError::FileAccess { path, source });
                }
            };
            reader
                .decode()
                .map_err(|source| ImageProcessingError::Decode { path, source })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut canvas = RgbaImage::from_pixel(
        options.width.max(1),
        options.height.max(1),
        Rgba([12, 12, 14, 255]),
    );
    if options.thumb_layout && inputs.len() > 1 {
        draw_thumb_collage(&mut canvas, &inputs);
    } else {
        draw_grid_collage(&mut canvas, &inputs);
    }
    Ok(DynamicImage::ImageRgba8(canvas))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn draw_grid_collage(canvas: &mut RgbaImage, inputs: &[DynamicImage]) {
    let columns = (inputs.len() as f64).sqrt().ceil() as u32;
    let rows = u32::try_from(inputs.len().div_ceil(usize::try_from(columns).unwrap_or(1)))
        .unwrap_or(u32::MAX)
        .max(1);
    let cell_width = canvas.width() / columns.max(1);
    let cell_height = canvas.height() / rows.max(1);
    if cell_width == 0 || cell_height == 0 {
        return;
    }
    for (index, input) in inputs.iter().enumerate() {
        let row = u32::try_from(index / usize::try_from(columns).unwrap_or(1)).unwrap_or(0);
        let column = u32::try_from(index % usize::try_from(columns).unwrap_or(1)).unwrap_or(0);
        let x = column * cell_width;
        let y = row * cell_height;
        let scaled = fill_crop(input, cell_width, cell_height);
        image::imageops::overlay(canvas, &scaled, i64::from(x), i64::from(y));
    }
}

fn draw_thumb_collage(canvas: &mut RgbaImage, inputs: &[DynamicImage]) {
    let first_width = canvas.width() * 2 / 3;
    let remaining_width = canvas.width() - first_width;
    let first = fill_crop(&inputs[0], first_width, canvas.height());
    image::imageops::overlay(canvas, &first, 0, 0);
    let rest = &inputs[1..];
    let cell_height = canvas.height() / u32::try_from(rest.len()).unwrap_or(1).max(1);
    for (index, input) in rest.iter().enumerate() {
        let cell = fill_crop(input, remaining_width, cell_height);
        image::imageops::overlay(
            canvas,
            &cell,
            i64::from(first_width),
            i64::from(u32::try_from(index).unwrap_or(u32::MAX) * cell_height),
        );
    }
}

fn fill_crop(input: &DynamicImage, width: u32, height: u32) -> DynamicImage {
    if width == 0 || height == 0 {
        return DynamicImage::ImageRgba8(RgbaImage::new(1, 1));
    }
    let (source_width, source_height) = input.dimensions();
    let scale = Ratio::new(width, source_width).max(Ratio::new(height, source_height));
    let scaled_width = scale.scale_ceil(source_width);
    let scaled_height = scale.scale_ceil(source_height);
    let resized = input.resize_exact(scaled_width, scaled_height, FilterType::Lanczos3);
    let x = (scaled_width - width) / 2;
    let y = (scaled_height - height) / 2;
    resized.crop_imm(x, y, width, height)
}

#[derive(Clone, Debug)]
struct NormalizedRequest {
    width: Option<u32>,
    height: Option<u32>,
    max_width: Option<u32>,
    max_height: Option<u32>,
    fill_width: Option<u32>,
    fill_height: Option<u32>,
    quality: u8,
    format: Option<ImageFormat>,
    supported_formats: Vec<ImageFormat>,
    blur: Option<u32>,
    background_color: Option<Rgba<u8>>,
    foreground_opacity: Option<f64>,
    percent_played: Option<f64>,
    unplayed_count: Option<i32>,
}

impl NormalizedRequest {
    fn new(request: ImageProcessingRequest) -> Result<Self, ImageProcessingError> {
        if !(1..=100).contains(&request.quality) {
            return Err(ImageProcessingError::InvalidQuality(request.quality));
        }
        if request
            .percent_played
            .is_some_and(|value| !value.is_finite())
        {
            return Err(ImageProcessingError::InvalidPercentPlayed);
        }

        let percent_played = request
            .percent_played
            .filter(|value| *value > 0.0 && *value < 100.0);
        let unplayed_count = request
            .unplayed_count
            .filter(|value| *value > 0 && percent_played.is_none());
        let foreground_opacity = request
            .foreground_layer
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.parse::<f64>().unwrap_or(0.4).clamp(0.0, 1.0));

        let background_color = request
            .background_color
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(parse_color)
            .transpose()?;

        Ok(Self {
            width: nonzero(request.width),
            height: nonzero(request.height),
            max_width: nonzero(request.max_width),
            max_height: nonzero(request.max_height),
            fill_width: nonzero(request.fill_width),
            fill_height: nonzero(request.fill_height),
            quality: request.quality,
            format: request.format,
            supported_formats: request.supported_formats,
            blur: nonzero(request.blur),
            background_color,
            foreground_opacity,
            percent_played,
            unplayed_count,
        })
    }

    fn can_return_original(
        &self,
        source: &ImageSource,
        source_format: Option<ImageFormat>,
    ) -> bool {
        let Some(source_format) = source_format else {
            return false;
        };
        if !self.accepts_source_format(source_format)
            || self.quality < 90
            || self.blur.is_some()
            || self.background_color.is_some()
            || self.foreground_opacity.is_some()
            || self.percent_played.is_some()
            || self.unplayed_count.is_some()
        {
            return false;
        }

        let dimensions = source.width.zip(source.height);
        dimensions.is_some_and(|(width, height)| {
            self.width.is_none_or(|requested| requested == width)
                && self.height.is_none_or(|requested| requested == height)
                && self.max_width.is_none_or(|maximum| width <= maximum)
                && self.max_height.is_none_or(|maximum| height <= maximum)
                && self.fill_width.is_none_or(|fill_width| width <= fill_width)
                && self
                    .fill_height
                    .is_none_or(|fill_height| height <= fill_height)
        }) || (dimensions.is_none()
            && self.width.is_none()
            && self.height.is_none()
            && self.max_width.is_none()
            && self.max_height.is_none())
    }

    fn accepts_source_format(&self, source_format: ImageFormat) -> bool {
        (validate_output_format(source_format).is_ok()
            || source_format == ImageFormat::Svg && self.format == Some(ImageFormat::Svg))
            && self.format.map_or_else(
                || self.supported_formats.contains(&source_format),
                |format| format == source_format,
            )
    }

    fn output_format(
        &self,
        source_format: Option<ImageFormat>,
    ) -> Result<ImageFormat, ImageProcessingError> {
        if let Some(format) = self.format {
            validate_output_format(format)?;
            return Ok(format);
        }
        if self.supported_formats.contains(&ImageFormat::Webp) {
            return Ok(ImageFormat::Webp);
        }
        if source_format.is_some_and(format_requires_transparency)
            && self.supported_formats.contains(&ImageFormat::Png)
        {
            return Ok(ImageFormat::Png);
        }
        self.supported_formats
            .iter()
            .copied()
            .find(|format| validate_output_format(*format).is_ok())
            .ok_or(ImageProcessingError::NoSupportedOutputFormat)
    }
}

const fn format_requires_transparency(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Gif | ImageFormat::Png | ImageFormat::Svg | ImageFormat::Webp
    )
}

const fn nonzero(value: Option<u32>) -> Option<u32> {
    match value {
        Some(0) | None => None,
        Some(value) => Some(value),
    }
}

async fn ensure_source_exists(path: &Path) -> Result<(), ImageProcessingError> {
    tokio::fs::metadata(path)
        .await
        .map(|_| ())
        .map_err(|source| ImageProcessingError::FileAccess {
            path: path.to_path_buf(),
            source,
        })
}

async fn cached_result(
    path: &Path,
    format: ImageFormat,
) -> Result<Option<ProcessedImage>, ImageProcessingError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => {
            let date_modified =
                metadata
                    .modified()
                    .map_err(|source| ImageProcessingError::FileAccess {
                        path: path.to_path_buf(),
                        source,
                    })?;
            Ok(Some(ProcessedImage {
                path: path.to_path_buf(),
                mime_type: format.mime_type(),
                date_modified,
            }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ImageProcessingError::FileAccess {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn cache_path(
    cache_directory: &Path,
    source: &ImageSource,
    request: &NormalizedRequest,
    format: ImageFormat,
) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update([CACHE_VERSION]);
    hash.update(source.path.to_string_lossy().as_bytes());
    hash_system_time(&mut hash, source.date_modified);
    hash_option(&mut hash, source.width);
    hash_option(&mut hash, source.height);
    hash_option(&mut hash, request.width);
    hash_option(&mut hash, request.height);
    hash_option(&mut hash, request.max_width);
    hash_option(&mut hash, request.max_height);
    hash_option(&mut hash, request.fill_width);
    hash_option(&mut hash, request.fill_height);
    hash.update([request.quality, format as u8]);
    hash_option(&mut hash, request.blur);
    if let Some(color) = request.background_color {
        hash.update([1]);
        hash.update(color.0);
    } else {
        hash.update([0]);
    }
    hash.update(
        request
            .foreground_opacity
            .map_or(0, f64::to_bits)
            .to_le_bytes(),
    );
    hash.update(request.percent_played.map_or(0, f64::to_bits).to_le_bytes());
    hash.update(request.unplayed_count.unwrap_or_default().to_le_bytes());
    let digest = format!("{:x}", hash.finalize());
    let extension = format.extension().trim_start_matches('.');
    cache_directory
        .join(&digest[..2])
        .join(format!("{digest}.{extension}"))
}

fn hash_system_time(hash: &mut Sha256, value: SystemTime) {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            hash.update([0]);
            hash.update(duration.as_secs().to_le_bytes());
            hash.update(duration.subsec_nanos().to_le_bytes());
        }
        Err(error) => {
            hash.update([1]);
            hash.update(error.duration().as_secs().to_le_bytes());
            hash.update(error.duration().subsec_nanos().to_le_bytes());
        }
    }
}

fn hash_option(hash: &mut Sha256, value: Option<u32>) {
    if let Some(value) = value {
        hash.update([1]);
        hash.update(value.to_le_bytes());
    } else {
        hash.update([0]);
    }
}

fn process_to_cache(
    source_path: &Path,
    cache_path: &Path,
    request: &NormalizedRequest,
    output_format: ImageFormat,
) -> Result<(), ImageProcessingError> {
    let reader =
        ImageReader::open(source_path).map_err(|source| ImageProcessingError::FileAccess {
            path: source_path.to_path_buf(),
            source,
        })?;
    let reader =
        reader
            .with_guessed_format()
            .map_err(|source| ImageProcessingError::FileAccess {
                path: source_path.to_path_buf(),
                source,
            })?;
    if reader.format().is_none() {
        return Err(ImageProcessingError::UnknownSourceFormat(
            source_path.to_path_buf(),
        ));
    }
    let mut image = reader
        .decode()
        .map_err(|source| ImageProcessingError::Decode {
            path: source_path.to_path_buf(),
            source,
        })?;

    image = resize_image(image, request);
    if let Some(sigma) = request.blur {
        image = image.blur(blur_sigma(sigma));
    }
    if let Some(background) = request.background_color {
        image = apply_background(&image, background);
    }
    if let Some(opacity) = request.foreground_opacity {
        image = apply_foreground(&image, opacity);
    }
    if let Some(percent) = request.percent_played {
        image = draw_percent_played(&image, percent);
    } else if let Some(count) = request.unplayed_count {
        image = draw_unplayed_count(&image, count);
    }

    let parent = cache_path
        .parent()
        .expect("cache files always have a parent");
    fs::create_dir_all(parent).map_err(|source| ImageProcessingError::FileAccess {
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary_path = cache_path.with_extension(format!(
        "{}.{}.tmp",
        output_format.extension().trim_start_matches('.'),
        unique_suffix()
    ));
    let icc_profile = extract_icc_profile_from_file(source_path).unwrap_or(None);
    let result = encode_image(
        &image,
        &temporary_path,
        request.quality,
        output_format,
        icc_profile.as_deref(),
    )
    .and_then(|()| persist_cache(&temporary_path, cache_path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[allow(clippy::cast_precision_loss)]
fn blur_sigma(sigma: u32) -> f32 {
    sigma as f32
}

fn resize_image(mut image: DynamicImage, request: &NormalizedRequest) -> DynamicImage {
    let (width, height) = image.dimensions();
    let (target_width, target_height) = new_image_size(width, height, request);
    if (target_width, target_height) != (width, height) {
        image = image.resize_exact(target_width, target_height, FilterType::Lanczos3);
    }
    image
}

fn new_image_size(
    source_width: u32,
    source_height: u32,
    request: &NormalizedRequest,
) -> (u32, u32) {
    let (width, height) = drawing_utils_resize(
        source_width,
        source_height,
        request.width,
        request.height,
        request.max_width,
        request.max_height,
    );
    let (width, height) = resize_fill(width, height, request.fill_width, request.fill_height);
    scale_down_to_fit(width, height, source_width, source_height)
}

fn drawing_utils_resize(
    width: u32,
    height: u32,
    requested_width: Option<u32>,
    requested_height: Option<u32>,
    max_width: Option<u32>,
    max_height: Option<u32>,
) -> (u32, u32) {
    let mut new_width = width;
    let mut new_height = height;
    if let (Some(requested_width), Some(requested_height)) = (requested_width, requested_height) {
        new_width = requested_width;
        new_height = requested_height;
    } else if let Some(requested_height) = requested_height {
        new_width = rounded_int(f64::from(requested_height) / f64::from(height) * f64::from(width));
        new_height = requested_height;
    } else if let Some(requested_width) = requested_width {
        new_height = rounded_int(f64::from(requested_width) / f64::from(width) * f64::from(height));
        new_width = requested_width;
    }

    if let Some(maximum_height) = max_height.filter(|maximum| *maximum < new_height) {
        let current_height = new_height;
        let current_width = new_width;
        new_width = rounded_int(
            f64::from(maximum_height) / f64::from(current_height) * f64::from(current_width),
        );
        new_height = maximum_height;
    }
    if let Some(maximum_width) = max_width.filter(|maximum| *maximum < new_width) {
        let current_height = new_height;
        let current_width = new_width;
        new_height = rounded_int(
            f64::from(maximum_width) / f64::from(current_width) * f64::from(current_height),
        );
        new_width = maximum_width;
    }
    (new_width, new_height)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn resize_fill(
    width: u32,
    height: u32,
    fill_width: Option<u32>,
    fill_height: Option<u32>,
) -> (u32, u32) {
    if fill_width.is_none() && fill_height.is_none() {
        return (width, height);
    }

    let fill_width = fill_width.unwrap_or(1);
    let fill_height = fill_height.unwrap_or(1);
    let width_ratio = f64::from(width) / f64::from(fill_width);
    let height_ratio = f64::from(height) / f64::from(fill_height);
    let scale_ratio = width_ratio.min(height_ratio);
    if scale_ratio < 1.0 {
        return (width, height);
    }

    (
        (f64::from(width) / scale_ratio).ceil().max(1.0) as u32,
        (f64::from(height) / scale_ratio).ceil().max(1.0) as u32,
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scale_down_to_fit(
    width: u32,
    height: u32,
    bounding_width: u32,
    bounding_height: u32,
) -> (u32, u32) {
    if width == 0 || height == 0 || bounding_width == 0 || bounding_height == 0 {
        return (width, height);
    }

    let width_ratio = f64::from(width) / f64::from(bounding_width);
    let height_ratio = f64::from(height) / f64::from(bounding_height);
    let scale_ratio = width_ratio.max(height_ratio);
    if scale_ratio <= 1.0 {
        return (width, height);
    }

    (
        (f64::from(width) / scale_ratio)
            .round_ties_even()
            .clamp(1.0, f64::from(bounding_width)) as u32,
        (f64::from(height) / scale_ratio)
            .round_ties_even()
            .clamp(1.0, f64::from(bounding_height)) as u32,
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_int(value: f64) -> u32 {
    value.round_ties_even().clamp(1.0, f64::from(u32::MAX)) as u32
}

#[derive(Clone, Copy)]
struct Ratio {
    numerator: u32,
    denominator: u32,
}

impl Ratio {
    const fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    fn max(self, other: Self) -> Self {
        if u128::from(self.numerator) * u128::from(other.denominator)
            >= u128::from(other.numerator) * u128::from(self.denominator)
        {
            self
        } else {
            other
        }
    }

    fn scale_ceil(self, dimension: u32) -> u32 {
        let numerator = u64::from(dimension) * u64::from(self.numerator);
        let rounded = numerator.div_ceil(u64::from(self.denominator));
        u32::try_from(rounded).unwrap_or(u32::MAX).max(1)
    }
}

fn apply_background(image: &DynamicImage, color: Rgba<u8>) -> DynamicImage {
    let foreground = image.to_rgba8();
    let mut canvas = RgbaImage::from_pixel(foreground.width(), foreground.height(), color);
    image::imageops::overlay(&mut canvas, &foreground, 0, 0);
    DynamicImage::ImageRgba8(canvas)
}

fn apply_foreground(image: &DynamicImage, opacity: f64) -> DynamicImage {
    let mut canvas = image.to_rgba8();
    let alpha = rounded_u8((1.0 - opacity) * 255.0);
    let overlay = RgbaImage::from_pixel(canvas.width(), canvas.height(), Rgba([0, 0, 0, alpha]));
    image::imageops::overlay(&mut canvas, &overlay, 0, 0);
    DynamicImage::ImageRgba8(canvas)
}

fn draw_percent_played(image: &DynamicImage, percent: f64) -> DynamicImage {
    let mut canvas = image.to_rgba8();
    let width = canvas.width().saturating_sub(1);
    let bottom = canvas.height().saturating_sub(1);
    let top = bottom.saturating_sub(8);
    for y in top..bottom {
        for x in 0..width {
            canvas.put_pixel(x, y, Rgba([0, 0, 0, 0x99]));
        }
    }
    let played_width = rounded_u32((f64::from(width) * percent) / 100.0, width);
    for y in top..bottom {
        for x in 0..played_width {
            canvas.put_pixel(x, y, Rgba([0, 0xa4, 0xdc, 0xff]));
        }
    }
    DynamicImage::ImageRgba8(canvas)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_u8(value: f64) -> u8 {
    value.round().clamp(0.0, f64::from(u8::MAX)) as u8
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_u32(value: f64, maximum: u32) -> u32 {
    value.round().clamp(0.0, f64::from(maximum)) as u32
}

fn draw_unplayed_count(image: &DynamicImage, count: i32) -> DynamicImage {
    let mut canvas = image.to_rgba8();
    let center_x = canvas.width().saturating_sub(38);
    let center_y = 38_u32.min(canvas.height().saturating_sub(1));
    let radius = 20_i64;
    let min_x = center_x.saturating_sub(20);
    let max_x = center_x
        .saturating_add(20)
        .min(canvas.width().saturating_sub(1));
    let min_y = center_y.saturating_sub(20);
    let max_y = center_y
        .saturating_add(20)
        .min(canvas.height().saturating_sub(1));
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = i64::from(x) - i64::from(center_x);
            let dy = i64::from(y) - i64::from(center_y);
            if dx * dx + dy * dy <= radius * radius {
                canvas.put_pixel(x, y, Rgba([0, 0xa4, 0xdc, 0xcc]));
            }
        }
    }
    draw_count_glyphs(&mut canvas, center_x, center_y, &count.to_string());
    DynamicImage::ImageRgba8(canvas)
}

fn draw_count_glyphs(canvas: &mut RgbaImage, center_x: u32, center_y: u32, text: &str) {
    const DIGITS: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111],
        [0b010, 0b110, 0b010, 0b010, 0b111],
        [0b111, 0b001, 0b111, 0b100, 0b111],
        [0b111, 0b001, 0b111, 0b001, 0b111],
        [0b101, 0b101, 0b111, 0b001, 0b001],
        [0b111, 0b100, 0b111, 0b001, 0b111],
        [0b111, 0b100, 0b111, 0b101, 0b111],
        [0b111, 0b001, 0b010, 0b010, 0b010],
        [0b111, 0b101, 0b111, 0b101, 0b111],
        [0b111, 0b101, 0b111, 0b001, 0b111],
    ];
    let digits = text
        .bytes()
        .filter_map(|digit| digit.checked_sub(b'0').map(usize::from))
        .take(3)
        .collect::<Vec<_>>();
    let scale = if digits.len() >= 3 { 2 } else { 3 };
    let glyph_width = 3 * scale;
    let spacing = scale;
    let total_width = digits
        .len()
        .saturating_mul(glyph_width + spacing)
        .saturating_sub(spacing);
    let start_x = center_x.saturating_sub(u32::try_from(total_width / 2).unwrap_or_default());
    let start_y = center_y.saturating_sub(u32::try_from(5 * scale / 2).unwrap_or_default());
    for (position, digit) in digits.into_iter().enumerate() {
        for (row, bits) in DIGITS[digit].into_iter().enumerate() {
            for column in 0..3 {
                if bits & (1 << (2 - column)) == 0 {
                    continue;
                }
                for offset_y in 0..scale {
                    for offset_x in 0..scale {
                        let x_offset =
                            position * (glyph_width + spacing) + column * scale + offset_x;
                        let x = start_x + u32::try_from(x_offset).unwrap_or_default();
                        let y = start_y + u32::try_from(row * scale + offset_y).unwrap_or_default();
                        if x < canvas.width() && y < canvas.height() {
                            canvas.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                        }
                    }
                }
            }
        }
    }
}

/// Encodes `image` with the encoder settings Jellyfin uses and, for JPEG
/// output, carries the source's embedded ICC profile over to the result.
///
/// Grayscale sources keep their `L8` colour type so a monochrome image is not
/// expanded to RGB on the way out.
fn encode_image(
    image: &DynamicImage,
    path: &Path,
    quality: u8,
    format: ImageFormat,
    icc_profile: Option<&[u8]>,
) -> Result<(), ImageProcessingError> {
    let mut encoded = Vec::new();
    let encode_result = match format {
        ImageFormat::Jpg => match image {
            DynamicImage::ImageLuma8(luma) => {
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
                    .write_image(
                        luma.as_raw(),
                        luma.width(),
                        luma.height(),
                        image::ExtendedColorType::L8,
                    )
            }
            DynamicImage::ImageLumaA8(la) => {
                let luma = la.pixels().map(|pixel| pixel.0[0]).collect::<Vec<_>>();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
                    .write_image(&luma, la.width(), la.height(), image::ExtendedColorType::L8)
            }
            _ => {
                let rgb = image.to_rgb8();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
                    .write_image(
                        rgb.as_raw(),
                        rgb.width(),
                        rgb.height(),
                        image::ExtendedColorType::Rgb8,
                    )
            }
        },
        format => image.write_to(&mut Cursor::new(&mut encoded), decoder_format(format)?),
    };
    encode_result.map_err(|source| ImageProcessingError::Encode {
        path: path.to_path_buf(),
        source,
    })?;

    if format == ImageFormat::Jpg
        && let Some(profile) = icc_profile
    {
        embed_icc_profile(&mut encoded, profile);
    }

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| ImageProcessingError::FileAccess {
            path: path.to_path_buf(),
            source,
        })?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(&encoded)
        .map_err(|source| ImageProcessingError::FileAccess {
            path: path.to_path_buf(),
            source,
        })?;
    writer
        .flush()
        .map_err(|source| ImageProcessingError::FileAccess {
            path: path.to_path_buf(),
            source,
        })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| ImageProcessingError::FileAccess {
            path: path.to_path_buf(),
            source,
        })
}

fn persist_cache(temporary_path: &Path, cache_path: &Path) -> Result<(), ImageProcessingError> {
    match fs::rename(temporary_path, cache_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(temporary_path).map_err(|source| ImageProcessingError::FileAccess {
                path: temporary_path.to_path_buf(),
                source,
            })
        }
        Err(source) => Err(ImageProcessingError::FileAccess {
            path: cache_path.to_path_buf(),
            source,
        }),
    }
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn validate_output_format(format: ImageFormat) -> Result<(), ImageProcessingError> {
    match format {
        ImageFormat::Bmp
        | ImageFormat::Gif
        | ImageFormat::Jpg
        | ImageFormat::Png
        | ImageFormat::Webp => Ok(()),
        ImageFormat::Svg => Err(ImageProcessingError::UnsupportedOutputFormat(format)),
    }
}

fn decoder_format(format: ImageFormat) -> Result<DecoderFormat, ImageProcessingError> {
    match format {
        ImageFormat::Bmp => Ok(DecoderFormat::Bmp),
        ImageFormat::Gif => Ok(DecoderFormat::Gif),
        ImageFormat::Jpg => Ok(DecoderFormat::Jpeg),
        ImageFormat::Png => Ok(DecoderFormat::Png),
        ImageFormat::Webp => Ok(DecoderFormat::WebP),
        ImageFormat::Svg => Err(ImageProcessingError::UnsupportedOutputFormat(format)),
    }
}

fn format_from_path(path: &Path) -> Option<ImageFormat> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "bmp" => Some(ImageFormat::Bmp),
        "gif" => Some(ImageFormat::Gif),
        "jpg" | "jpeg" => Some(ImageFormat::Jpg),
        "png" => Some(ImageFormat::Png),
        "webp" => Some(ImageFormat::Webp),
        "svg" => Some(ImageFormat::Svg),
        _ => None,
    }
}

/// Extracts the raw ICC color profile bytes embedded in an image, if present.
#[must_use]
pub fn extract_icc_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    // 1. JPEG APP2 ICC_PROFILE
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut offset = 2;
        let mut chunks: std::collections::BTreeMap<u8, Vec<u8>> = std::collections::BTreeMap::new();
        while offset + 4 <= bytes.len() {
            if bytes[offset] != 0xFF {
                break;
            }
            let marker = bytes[offset + 1];
            if marker == 0xDA || marker == 0xD9 {
                break;
            }
            let length = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
            if offset + 2 + length > bytes.len() {
                break;
            }
            let segment = &bytes[offset + 4..offset + 2 + length];
            if marker == 0xE2 && segment.len() >= 14 && segment.starts_with(b"ICC_PROFILE\0") {
                let seq_no = segment[12];
                let data = &segment[14..];
                chunks.insert(seq_no, data.to_vec());
            }
            offset += 2 + length;
        }
        if !chunks.is_empty() {
            let mut total = Vec::new();
            for (_, chunk) in chunks {
                total.extend_from_slice(&chunk);
            }
            return Some(total);
        }
    }

    // 2. PNG iCCP chunk
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        let mut offset = 8;
        while offset + 8 <= bytes.len() {
            let length = u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]) as usize;
            let chunk_type = &bytes[offset + 4..offset + 8];
            if offset + 12 + length > bytes.len() {
                break;
            }
            if chunk_type == b"iCCP" {
                let data = &bytes[offset + 8..offset + 8 + length];
                if let Some(null_pos) = data.iter().position(|&b| b == 0)
                    && null_pos + 2 < data.len()
                    && data[null_pos + 1] == 0
                {
                    let compressed = &data[null_pos + 2..];
                    if let Ok(decompressed) =
                        miniz_oxide::inflate::decompress_to_vec_zlib(compressed)
                    {
                        return Some(decompressed);
                    }
                }
            }
            offset += 12 + length;
        }
    }

    // 3. WebP ICCP chunk
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let mut offset = 12;
        while offset + 8 <= bytes.len() {
            let fourcc = &bytes[offset..offset + 4];
            let length = u32::from_le_bytes([
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]) as usize;
            if offset + 8 + length > bytes.len() {
                break;
            }
            let payload = &bytes[offset + 8..offset + 8 + length];
            if fourcc == b"ICCP" {
                return Some(payload.to_vec());
            }
            offset += 8 + ((length + 1) & !1);
        }
    }

    None
}

/// Re-embeds an ICC profile into an encoded JPEG as an APP2 segment.
///
/// The segment is inserted straight after the SOI marker. The ICC
/// specification caps a chunk's payload at just under 64 KiB, so larger
/// profiles are split across numbered chunks, exactly as encoders in the wild
/// write them.
pub fn embed_icc_profile(jpeg: &mut Vec<u8>, profile: &[u8]) {
    const MAX_CHUNK_PAYLOAD: usize = 65_519;

    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 || profile.is_empty() {
        return;
    }

    let parts = profile.chunks(MAX_CHUNK_PAYLOAD).collect::<Vec<_>>();
    let total_chunks = u8::try_from(parts.len()).unwrap_or(u8::MAX);

    let mut segments = Vec::with_capacity(parts.len());
    for (index, chunk) in parts.iter().enumerate() {
        let mut segment = Vec::with_capacity(chunk.len() + 14);
        segment.extend_from_slice(b"ICC_PROFILE\0");
        segment.push(u8::try_from(index + 1).unwrap_or(u8::MAX));
        segment.push(total_chunks);
        segment.extend_from_slice(chunk);
        segments.push(segment);
    }

    let extra = segments
        .iter()
        .map(|segment| segment.len() + 4)
        .sum::<usize>();
    let mut output = Vec::with_capacity(jpeg.len() + extra);
    output.extend_from_slice(&jpeg[..2]);
    for segment in &segments {
        // The length field counts its own two bytes plus the payload.
        let length = u16::try_from(segment.len() + 2).unwrap_or(u16::MAX);
        output.push(0xFF);
        output.push(0xE2);
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(segment);
    }
    output.extend_from_slice(&jpeg[2..]);
    *jpeg = output;
}

/// Extracts the raw ICC color profile bytes from a local image file.
///
/// # Errors
///
/// Returns an error when reading the file fails.
pub fn extract_icc_profile_from_file(
    path: impl AsRef<Path>,
) -> Result<Option<Vec<u8>>, std::io::Error> {
    let bytes = fs::read(path)?;
    Ok(extract_icc_profile(&bytes))
}

/// Returns whether the provided image represents a grayscale color space.
#[must_use]
pub const fn is_grayscale_image(image: &DynamicImage) -> bool {
    matches!(
        image,
        DynamicImage::ImageLuma8(_)
            | DynamicImage::ImageLuma16(_)
            | DynamicImage::ImageLumaA8(_)
            | DynamicImage::ImageLumaA16(_)
    )
}

fn parse_color(value: &str) -> Result<Rgba<u8>, ImageProcessingError> {
    let normalized = value.strip_prefix('#').unwrap_or(value);
    let color = match normalized.len() {
        3 => Rgba([
            duplicate_hex(&normalized[0..1])?,
            duplicate_hex(&normalized[1..2])?,
            duplicate_hex(&normalized[2..3])?,
            255,
        ]),
        4 => Rgba([
            duplicate_hex(&normalized[0..1])?,
            duplicate_hex(&normalized[1..2])?,
            duplicate_hex(&normalized[2..3])?,
            duplicate_hex(&normalized[3..4])?,
        ]),
        6 | 8 => {
            let mut channels = [0, 0, 0, 255];
            for (index, chunk) in normalized.as_bytes().chunks_exact(2).enumerate() {
                let chunk = std::str::from_utf8(chunk)
                    .map_err(|_| ImageProcessingError::InvalidBackgroundColor(value.into()))?;
                channels[index] = u8::from_str_radix(chunk, 16)
                    .map_err(|_| ImageProcessingError::InvalidBackgroundColor(value.into()))?;
            }
            Rgba(channels)
        }
        _ => {
            return Err(ImageProcessingError::InvalidBackgroundColor(value.into()));
        }
    };
    Ok(color)
}

fn duplicate_hex(value: &str) -> Result<u8, ImageProcessingError> {
    let digit = u8::from_str_radix(value, 16)
        .map_err(|_| ImageProcessingError::InvalidBackgroundColor(value.into()))?;
    Ok(digit * 17)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_cold_cache_requests_encode_once() {
        const REQUESTS: usize = 8;

        let directory = tempfile::tempdir().expect("temporary directory");
        let source_path = directory.path().join("source.png");
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(512, 288, Rgba([20, 40, 80, 255])))
            .save(&source_path)
            .expect("write source fixture");
        let date_modified = fs::metadata(&source_path)
            .and_then(|metadata| metadata.modified())
            .expect("source modification time");
        let source = ImageSource::new(source_path, date_modified).with_dimensions(512, 288);
        let request = ImageProcessingRequest {
            max_width: Some(256),
            ..ImageProcessingRequest::default()
        };
        let processor =
            ImageProcessor::new(directory.path().join("cache"), REQUESTS).expect("image processor");
        *processor
            .test_instrumentation
            .arrivals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(tokio::sync::Barrier::new(REQUESTS)));
        *processor
            .test_instrumentation
            .delay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(std::time::Duration::from_millis(100));

        let mut tasks = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let processor = processor.clone();
            let source = source.clone();
            let request = request.clone();
            tasks.push(tokio::spawn(async move {
                processor.process(source, request).await
            }));
        }

        let mut output_path = None;
        for task in tasks {
            let result = task.await.expect("processing task").expect("process image");
            if let Some(expected) = &output_path {
                assert_eq!(&result.path, expected);
            } else {
                output_path = Some(result.path);
            }
        }
        assert_eq!(
            processor
                .test_instrumentation
                .attempts
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "identical requests must share a single memory-heavy encode"
        );
        assert!(
            processor
                .encoding_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "completed cache keys must not accumulate in the lock registry"
        );
    }

    #[test]
    fn drawing_utils_maximums_scale_down_and_never_upscale() {
        assert_eq!(
            drawing_utils_resize(1920, 1080, None, None, Some(1280), Some(800)),
            (1280, 720)
        );
        assert_eq!(
            drawing_utils_resize(320, 180, None, None, Some(1280), Some(800)),
            (320, 180)
        );
    }

    #[test]
    fn parses_short_and_alpha_hex_colors() {
        assert_eq!(parse_color("#0f8").unwrap(), Rgba([0, 255, 136, 255]));
        assert_eq!(parse_color("11223344").unwrap(), Rgba([17, 34, 51, 68]));
    }

    #[test]
    fn blur_hash_components_follow_jellyfin_aspect_ratio_formula() {
        assert_eq!(blur_hash_components(1920, 1080), (6, 4));
        assert_eq!(blur_hash_components(1080, 1920), (4, 6));
        assert_eq!(blur_hash_components(1, 10_000), (1, 9));
        assert_eq!(blur_hash_components(10_000, 1), (9, 1));
    }
}
