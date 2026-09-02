use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::Stdio,
};

use image::{GenericImageView, ImageEncoder, ImageReader, imageops};
use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, NewTrickplayInfo, TrickplayInfo,
    TrickplayInfoRepository, TrickplayInfoStoreError,
};
use jellyfin_drawing::ImageInspectionError;
use jellyfin_model::{TrickplayInfoDto, configuration::TrickplayOptions};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use tokio::{fs, process::Command};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum TrickplayError {
    #[error(transparent)]
    Store(#[from] TrickplayInfoStoreError),
    #[error(transparent)]
    Catalog(#[from] BaseItemError),
    #[error(transparent)]
    Image(#[from] ImageInspectionError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to generate trickplay images: {stderr}")]
    Ffmpeg { stderr: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrickplayGenerationRequest {
    pub item_id: Uuid,
    pub source_path: String,
    pub width: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub interval_ms: i32,
    pub jpeg_quality: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePlan {
    pub input_count: usize,
    pub tile_count: usize,
    pub thumbnails_per_tile: usize,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
}

/// Reads trickplay metadata and maps it to Jellyfin's playlist and tile layout.
#[derive(Clone)]
pub struct TrickplayService {
    repository: TrickplayInfoRepository,
    items: BaseItemRepository,
    storage_directory: PathBuf,
}

impl TrickplayService {
    #[must_use]
    pub fn new(database: DatabaseConnection, storage_directory: impl Into<PathBuf>) -> Self {
        Self {
            repository: TrickplayInfoRepository::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            storage_directory: storage_directory.into(),
        }
    }

    /// Replaces the directory used by trickplay read/write operations.
    pub fn set_storage_directory(&mut self, storage_directory: impl Into<PathBuf>) {
        self.storage_directory = storage_directory.into();
    }

    /// Builds the official image-only HLS playlist for one resolution.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when trickplay metadata cannot be loaded.
    pub async fn playlist(
        &self,
        item_id: Uuid,
        width: i32,
        api_key: &str,
    ) -> Result<Option<String>, TrickplayError> {
        let Some(info) = self.repository.get(item_id, width).await? else {
            return Ok(None);
        };
        Ok(build_playlist(info, api_key))
    }

    /// Loads API manifests for multiple displayed items without N+1 queries.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when trickplay metadata cannot be loaded.
    pub async fn manifests_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<TrickplayManifests, TrickplayError> {
        Ok(self
            .repository
            .manifests_for_items(item_ids)
            .await?
            .into_iter()
            .map(|(display_item_id, sources)| {
                let sources = sources
                    .into_iter()
                    .map(|(source_id, resolutions)| {
                        let resolutions = resolutions
                            .into_iter()
                            .map(|(width, info)| (width, info_to_dto(info)))
                            .collect();
                        (source_id.simple().to_string(), resolutions)
                    })
                    .collect();
                (display_item_id, sources)
            })
            .collect())
    }

    /// Removes persisted metadata and internally managed tiles for one item.
    ///
    /// Metadata is deleted first so a failed best-effort file cleanup cannot
    /// leave stale playlists or manifests visible to clients.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when trickplay metadata cannot be deleted.
    pub async fn delete_data(&self, item_id: Uuid) -> Result<bool, TrickplayError> {
        let deleted = self.repository.delete_for_item(item_id).await?;
        let directory = internal_item_directory(&self.storage_directory, item_id);
        match fs::remove_dir_all(&directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                path = %directory.display(),
                %error,
                "trickplay metadata was deleted but managed tiles could not be removed"
            ),
        }
        Ok(deleted)
    }

    /// Discovers managed tile directories and reconciles their metadata.
    ///
    /// Existing database rows win over inferred values. Directories with JPEG
    /// files but unreadable image headers still count as present so discovery
    /// never removes metadata merely because one tile is temporarily corrupt.
    ///
    /// # Errors
    ///
    /// Returns file-system or persistence errors. Malformed directories and
    /// unsupported tile images are skipped independently.
    pub async fn discover_data(
        &self,
        item_id: Uuid,
        runtime_ticks: Option<i64>,
        configured_interval: i32,
    ) -> Result<Vec<TrickplayInfo>, TrickplayError> {
        let item_directory = internal_item_directory(&self.storage_directory, item_id);
        let mut present_widths = Vec::new();
        let mut discovered = Vec::new();
        let mut directories = match fs::read_dir(&item_directory).await {
            Ok(directories) => directories,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self
                    .repository
                    .synchronize_discovered(item_id, &[], &[])
                    .await?);
            }
            Err(error) => return Err(error.into()),
        };

        let mut entries = Vec::new();
        while let Some(entry) = directories.next_entry().await? {
            entries.push(entry);
        }
        entries.sort_unstable_by_key(tokio::fs::DirEntry::file_name);

        for entry in entries {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let Some((width, tile_width, tile_height)) =
                parse_resolution_directory(&entry.file_name().to_string_lossy())
            else {
                continue;
            };
            let tiles = jpeg_tiles(&entry.path()).await?;
            if tiles.is_empty() {
                continue;
            }
            present_widths.push(width);
            if let Some(info) = infer_discovered_info(
                width,
                tile_width,
                tile_height,
                &tiles,
                runtime_ticks,
                configured_interval,
            )
            .await
            {
                discovered.push(info);
            }
        }

        Ok(self
            .repository
            .synchronize_discovered(item_id, &present_widths, &discovered)
            .await?)
    }

    /// Resolves an internally stored JPEG tile without trusting path input.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when trickplay metadata cannot be loaded.
    pub async fn tile_path(
        &self,
        item_id: Uuid,
        width: i32,
        index: i32,
    ) -> Result<Option<PathBuf>, TrickplayError> {
        let Some(info) = self.repository.get(item_id, width).await? else {
            return Ok(None);
        };
        Ok(Some(internal_tile_path(
            &self.storage_directory,
            info,
            index,
        )))
    }

    /// Generates tiles for every configured resolution.
    ///
    /// Missing or absent sources are skipped, while generation failures for
    /// one resolution are logged and do not stop the remaining resolutions.
    ///
    /// # Errors
    ///
    /// Returns a catalog error when the item cannot be loaded.
    pub async fn generate_for_item(
        &self,
        item_id: Uuid,
        options: &TrickplayOptions,
        ffmpeg_path: impl AsRef<Path>,
        work_root: impl AsRef<Path>,
    ) -> Result<(), TrickplayError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(());
        };
        let Some(path) = item.path.filter(|path| !path.trim().is_empty()) else {
            return Ok(());
        };
        if !fs::try_exists(Path::new(&path)).await.unwrap_or_default() {
            return Ok(());
        }
        let interval = options.interval.max(1_000);
        let work_root = work_root.as_ref();
        for requested_width in options.width_resolutions.iter().copied() {
            let request = TrickplayGenerationRequest {
                item_id,
                source_path: path.clone(),
                width: even_width(requested_width),
                tile_width: options.tile_width.clamp(1, 255),
                tile_height: options.tile_height.clamp(1, 255),
                interval_ms: interval,
                jpeg_quality: options.jpeg_quality.clamp(1, 100),
            };
            if let Err(error) = self
                .generate_resolution(&request, &ffmpeg_path, work_root)
                .await
            {
                tracing::warn!(item_id = %item_id, width = request.width, %error, "skipped trickplay resolution");
            }
        }
        Ok(())
    }

    /// Generates tiles for all eligible library videos.
    ///
    /// Item failures are contained by [`Self::generate_for_item`].
    ///
    /// # Errors
    ///
    /// Returns a catalog error when the video query cannot be loaded.
    pub async fn generate_for_library(
        &self,
        options: &TrickplayOptions,
        ffmpeg_path: impl Into<PathBuf>,
        work_root: impl AsRef<Path>,
    ) -> Result<(), TrickplayError> {
        let query = BaseItemQuery {
            is_folder: Some(false),
            is_virtual_item: Some(false),
            media_types: vec!["Video".to_owned()],
            ..BaseItemQuery::default()
        };
        let page = self.items.query(&query).await?;
        let ffmpeg_path = ffmpeg_path.into();
        let work_root = work_root.as_ref();
        for item in page.items {
            self.generate_for_item(item.id, options, &ffmpeg_path, work_root)
                .await?;
        }
        Ok(())
    }

    /// Generates one trickplay resolution and persists its metadata.
    ///
    /// # Errors
    ///
    /// Returns I/O, process, image, or persistence failures for one resolution.
    pub async fn generate_resolution(
        &self,
        request: &TrickplayGenerationRequest,
        ffmpeg_path: impl AsRef<Path>,
        work_root: impl AsRef<Path>,
    ) -> Result<(), TrickplayError> {
        let work_directory = work_root
            .as_ref()
            .join(format!("trickplay-{}", Uuid::new_v4()));
        fs::create_dir_all(&work_directory).await?;
        let result = self
            .generate_resolution_in(request, ffmpeg_path, &work_directory)
            .await;
        let _ = fs::remove_dir_all(&work_directory).await;
        result
    }

    async fn generate_resolution_in(
        &self,
        request: &TrickplayGenerationRequest,
        ffmpeg_path: impl AsRef<Path>,
        work_directory: &Path,
    ) -> Result<(), TrickplayError> {
        let seconds = format_seconds(request.interval_ms);
        let command = Command::new(ffmpeg_path.as_ref())
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                &request.source_path,
                "-vf",
                &format!("fps=1/{seconds},scale={}:-2:flags=lanczos", request.width),
                "-vsync",
                "0",
                "-frame_pts",
                "1",
                "-q:v",
                &request.jpeg_quality.to_string(),
            ])
            .arg(work_directory.join("%08d.jpg"))
            .stdin(Stdio::null())
            .output()
            .await?;
        if !command.status.success() {
            return Err(TrickplayError::Ffmpeg {
                stderr: String::from_utf8_lossy(&command.stderr).into_owned(),
            });
        }
        let images = jpeg_tiles(work_directory).await?;
        if images.is_empty() {
            return Err(TrickplayError::Ffmpeg {
                stderr: "ffmpeg generated no thumbnails".to_owned(),
            });
        }
        let (_, thumbnail_height) = jellyfin_drawing::inspect_dimensions(&images[0]).await?;
        let plan = TilePlan::new(
            images.len(),
            request.width,
            thumbnail_height,
            request.tile_width,
            request.tile_height,
        );
        let output_directory = internal_item_directory(&self.storage_directory, request.item_id)
            .join(format!(
                "{} - {}x{}",
                request.width, plan.tile_width, plan.tile_height
            ));
        let _ = fs::remove_dir_all(&output_directory).await;
        fs::create_dir_all(&output_directory).await?;
        for tile_index in 0..plan.tile_count {
            let start = tile_index * plan.thumbnails_per_tile;
            let inputs = images[start..start + plan.thumbnails_per_tile].to_vec();
            let output = output_directory.join(format!("{tile_index}.jpg"));
            create_trickplay_tile(
                &inputs,
                output,
                plan.width,
                plan.height,
                plan.tile_width,
                plan.tile_height,
                request.jpeg_quality,
            )
            .await?;
        }
        let bandwidth = tile_bandwidth(
            &output_directory,
            plan.tile_width,
            plan.tile_height,
            request.interval_ms,
        )
        .await?;
        self.repository
            .upsert(
                request.item_id,
                NewTrickplayInfo {
                    width: i32::try_from(request.width).unwrap_or(0),
                    height: i32::try_from(plan.height).unwrap_or(0),
                    tile_width: i32::try_from(plan.tile_width).unwrap_or(0),
                    tile_height: i32::try_from(plan.tile_height).unwrap_or(0),
                    thumbnail_count: i32::try_from(images.len()).unwrap_or(0),
                    interval: request.interval_ms,
                    bandwidth: i32::try_from(bandwidth).unwrap_or(i32::MAX),
                },
            )
            .await?;
        Ok(())
    }
}

pub type TrickplayManifest = BTreeMap<String, BTreeMap<i32, TrickplayInfoDto>>;
pub type TrickplayManifests = HashMap<Uuid, TrickplayManifest>;

const fn info_to_dto(info: TrickplayInfo) -> TrickplayInfoDto {
    TrickplayInfoDto {
        width: info.width,
        height: info.height,
        tile_width: info.tile_width,
        tile_height: info.tile_height,
        thumbnail_count: info.thumbnail_count,
        interval: info.interval,
        bandwidth: info.bandwidth,
    }
}

fn build_playlist(info: TrickplayInfo, api_key: &str) -> Option<String> {
    if info.thumbnail_count <= 0 {
        return None;
    }

    let thumbnails_per_tile = i64::from(info.tile_width) * i64::from(info.tile_height);
    let thumbnail_count = i64::from(info.thumbnail_count);
    let tile_count = 1 + (thumbnail_count - 1) / thumbnails_per_tile;
    let item_id = info.item_id.simple();
    let mut playlist = String::with_capacity(256);
    playlist.push_str("#EXTM3U\n#EXT-X-TARGETDURATION:");
    playlist.push_str(&tile_count.to_string());
    playlist.push_str(
        "\n#EXT-X-VERSION:7\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-IMAGES-ONLY\n",
    );

    for index in 0..tile_count {
        let tile_thumbnails = if index == tile_count - 1 {
            thumbnail_count - index * thumbnails_per_tile
        } else {
            thumbnails_per_tile
        };
        let tile_duration_ms = i64::from(info.interval) * tile_thumbnails;
        playlist.push_str("#EXTINF:");
        playlist.push_str(&format_milliseconds(tile_duration_ms));
        playlist.push_str(",\n#EXT-X-TILES:RESOLUTION=");
        playlist.push_str(&info.width.to_string());
        playlist.push('x');
        playlist.push_str(&info.height.to_string());
        playlist.push_str(",LAYOUT=");
        playlist.push_str(&info.tile_width.to_string());
        playlist.push('x');
        playlist.push_str(&info.tile_height.to_string());
        playlist.push_str(",DURATION=");
        playlist.push_str(&format_milliseconds(i64::from(info.interval)));
        playlist.push('\n');
        playlist.push_str(&index.to_string());
        playlist.push_str(".jpg?MediaSourceId=");
        playlist.push_str(&item_id.to_string());
        playlist.push_str("&ApiKey=");
        playlist.push_str(api_key);
        playlist.push('\n');
    }
    playlist.push_str("#EXT-X-ENDLIST\n");
    Some(playlist)
}

fn format_milliseconds(milliseconds: i64) -> String {
    let seconds = milliseconds / 1_000;
    let remainder = milliseconds % 1_000;
    if remainder == 0 {
        return seconds.to_string();
    }
    let mut formatted = format!("{seconds}.{remainder:03}");
    while formatted.ends_with('0') {
        formatted.pop();
    }
    formatted
}

fn internal_tile_path(root: &Path, info: TrickplayInfo, index: i32) -> PathBuf {
    internal_item_directory(root, info.item_id)
        .join(format!(
            "{} - {}x{}",
            info.width, info.tile_width, info.tile_height
        ))
        .join(format!("{index}.jpg"))
}

fn internal_item_directory(root: &Path, item_id: Uuid) -> PathBuf {
    let id = item_id.hyphenated().to_string();
    root.join(&id[..2]).join(id)
}

impl TilePlan {
    #[must_use]
    pub fn new(
        input_count: usize,
        width: i32,
        thumbnail_height: u32,
        tile_width: i32,
        tile_height: i32,
    ) -> Self {
        let tile_width = usize::try_from(tile_width.max(1)).unwrap_or(1);
        let tile_height = usize::try_from(tile_height.max(1)).unwrap_or(1);
        let thumbnails_per_tile = tile_width.saturating_mul(tile_height);
        let tile_count = input_count.div_ceil(thumbnails_per_tile);
        let width = u32::try_from(width.max(2)).unwrap_or(2);
        Self {
            input_count,
            tile_count,
            thumbnails_per_tile,
            width,
            height: thumbnail_height.max(1),
            tile_width: u32::try_from(tile_width).unwrap_or(1),
            tile_height: u32::try_from(tile_height).unwrap_or(1),
        }
    }
}

fn even_width(width: i32) -> i32 {
    width.max(2) / 2 * 2
}

fn format_seconds(milliseconds: i32) -> String {
    let seconds = milliseconds / 1_000;
    let fraction = milliseconds % 1_000;
    format!("{seconds}.{fraction:03}")
}

fn tile_error(source: image::ImageError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, source)
}

async fn create_trickplay_tile(
    inputs: &[PathBuf],
    output: PathBuf,
    width: u32,
    height: u32,
    tile_width: u32,
    tile_height: u32,
    quality: i32,
) -> Result<(), std::io::Error> {
    let inputs = inputs.to_vec();
    let output = output.clone();
    tokio::task::spawn_blocking(move || {
        let mut canvas = image::RgbaImage::new(
            width.saturating_mul(tile_width),
            height.saturating_mul(tile_height),
        );
        for (index, path) in inputs.iter().enumerate() {
            let image = ImageReader::open(path)?
                .with_guessed_format()?
                .decode()
                .map_err(tile_error)?;
            if image.dimensions() != (width, height) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "trickplay thumbnail dimensions differ",
                ));
            }
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            let x = index % tile_width;
            let y = index / tile_width;
            imageops::overlay(
                &mut canvas,
                &image.to_rgba8(),
                i64::from(x.saturating_mul(width)),
                i64::from(y.saturating_mul(height)),
            );
        }
        let mut encoded = Vec::new();
        let rgb = image::DynamicImage::ImageRgba8(canvas).to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut encoded,
            u8::try_from(quality.clamp(1, 100)).unwrap_or(90),
        )
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(tile_error)?;
        std::fs::write(output, encoded)?;
        Ok(())
    })
    .await?
}

async fn tile_bandwidth(
    output_directory: &Path,
    tile_width: u32,
    tile_height: u32,
    interval_ms: i32,
) -> Result<i64, std::io::Error> {
    let thumbnails_per_tile = i64::from(tile_width).saturating_mul(i64::from(tile_height));
    let interval = i64::from(interval_ms.max(1_000));
    let mut entries = fs::read_dir(output_directory).await?;
    let mut bandwidth = 0_i64;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let bytes = i64::try_from(fs::metadata(entry.path()).await?.len()).unwrap_or(i64::MAX);
        let rate = bytes
            .saturating_mul(8_000)
            .checked_div(thumbnails_per_tile.saturating_mul(interval))
            .unwrap_or(i64::MAX);
        bandwidth = bandwidth.max(rate);
    }
    Ok(bandwidth)
}

fn parse_resolution_directory(name: &str) -> Option<(i32, i32, i32)> {
    let (width, layout) = name.split_once(" - ")?;
    let (tile_width, tile_height) = layout.split_once('x')?;
    if ![width, tile_width, tile_height]
        .iter()
        .all(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let values = (
        width.parse::<i32>().ok()?,
        tile_width.parse::<i32>().ok()?,
        tile_height.parse::<i32>().ok()?,
    );
    (values.0 > 0 && values.1 > 0 && values.2 > 0).then_some(values)
}

async fn jpeg_tiles(directory: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut entries = fs::read_dir(directory).await?;
    let mut tiles = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jpg")
        {
            tiles.push(entry.path());
        }
    }
    tiles.sort_unstable();
    Ok(tiles)
}

async fn infer_discovered_info(
    width: i32,
    tile_width: i32,
    tile_height: i32,
    tiles: &[PathBuf],
    runtime_ticks: Option<i64>,
    configured_interval: i32,
) -> Option<NewTrickplayInfo> {
    let (_, image_height) = jellyfin_drawing::inspect_dimensions(tiles.first()?)
        .await
        .ok()?;
    let tiles_len = i64::try_from(tiles.len()).ok()?;
    let thumbnails_per_tile = i64::from(tile_width).checked_mul(i64::from(tile_height))?;
    let max_thumbnails = tiles_len.checked_mul(thumbnails_per_tile)?;
    let min_thumbnails = if tiles_len > 1 {
        (tiles_len - 1)
            .checked_mul(thumbnails_per_tile)?
            .checked_add(1)?
    } else {
        1
    };
    let configured_interval = i64::from(configured_interval.max(1_000));
    let (interval, thumbnail_count) = runtime_ticks.filter(|ticks| *ticks > 0).map_or(
        (configured_interval, max_thumbnails),
        |ticks| {
            let runtime_ms = ticks / 10_000;
            let min_interval = ceil_div(runtime_ms, max_thumbnails).max(1_000);
            let max_interval = (runtime_ms / min_thumbnails).max(min_interval);
            let interval = if (min_interval..=max_interval).contains(&configured_interval) {
                configured_interval
            } else {
                round_ratio_ties_even(min_interval + max_interval, 2_000)
                    .saturating_mul(1_000)
                    .clamp(min_interval, max_interval)
            };
            let count =
                round_ratio_ties_even(runtime_ms, interval).clamp(min_thumbnails, max_thumbnails);
            (interval, count)
        },
    );
    let height = ceil_div(i64::from(image_height), i64::from(tile_height)).max(1);
    let mut bandwidth = 0_i64;
    for tile in tiles {
        let bytes = i64::try_from(fs::metadata(tile).await.ok()?.len()).ok()?;
        let bits_per_second = ceil_div(
            bytes.checked_mul(8_000)?,
            thumbnails_per_tile.checked_mul(interval)?,
        );
        bandwidth = bandwidth.max(bits_per_second);
    }

    Some(NewTrickplayInfo {
        width,
        height: i32::try_from(height).ok()?,
        tile_width,
        tile_height,
        thumbnail_count: i32::try_from(thumbnail_count).ok()?,
        interval: i32::try_from(interval).ok()?,
        bandwidth: i32::try_from(bandwidth).ok()?,
    })
}

const fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    numerator / denominator + if numerator % denominator == 0 { 0 } else { 1 }
}

const fn round_ratio_ties_even(numerator: i64, denominator: i64) -> i64 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let doubled = remainder * 2;
    if doubled < denominator || (doubled == denominator && quotient % 2 == 0) {
        quotient
    } else {
        quotient + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM_ID: Uuid = Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff);

    #[test]
    fn playlist_matches_official_partial_tile_contract() {
        let playlist = build_playlist(
            TrickplayInfo {
                item_id: ITEM_ID,
                width: 320,
                height: 180,
                tile_width: 2,
                tile_height: 2,
                thumbnail_count: 6,
                interval: 1_500,
                bandwidth: 22_000,
            },
            "abc",
        )
        .unwrap();
        assert_eq!(
            playlist,
            "#EXTM3U\n\
#EXT-X-TARGETDURATION:2\n\
#EXT-X-VERSION:7\n\
#EXT-X-MEDIA-SEQUENCE:1\n\
#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXT-X-IMAGES-ONLY\n\
#EXTINF:6,\n\
#EXT-X-TILES:RESOLUTION=320x180,LAYOUT=2x2,DURATION=1.5\n\
0.jpg?MediaSourceId=00112233445566778899aabbccddeeff&ApiKey=abc\n\
#EXTINF:3,\n\
#EXT-X-TILES:RESOLUTION=320x180,LAYOUT=2x2,DURATION=1.5\n\
1.jpg?MediaSourceId=00112233445566778899aabbccddeeff&ApiKey=abc\n\
#EXT-X-ENDLIST\n"
        );
    }

    #[test]
    fn empty_metadata_has_no_playlist_and_path_uses_hyphenated_id() {
        let info = TrickplayInfo {
            item_id: ITEM_ID,
            width: 320,
            height: 180,
            tile_width: 10,
            tile_height: 10,
            thumbnail_count: 0,
            interval: 10_000,
            bandwidth: 0,
        };
        assert!(build_playlist(info, "").is_none());
        assert_eq!(
            internal_tile_path(Path::new("programdata/trickplay"), info, -1),
            PathBuf::from(
                "programdata/trickplay/00/00112233-4455-6677-8899-aabbccddeeff/320 - 10x10/-1.jpg"
            )
        );
    }

    #[test]
    fn discovery_directory_parser_is_exact_and_positive() {
        assert_eq!(parse_resolution_directory("320 - 10x8"), Some((320, 10, 8)));
        for invalid in [
            "320-10x8",
            "320 - 10x8x2",
            "0 - 10x8",
            "320 - 0x8",
            "320 - 10x0",
            "width - 10x8",
            "+320 - 10x8",
        ] {
            assert_eq!(parse_resolution_directory(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn discovery_integer_rounding_matches_official_bounds() {
        assert_eq!(ceil_div(10_001, 6), 1_667);
        assert_eq!(round_ratio_ties_even(2_500, 1_000), 2);
        assert_eq!(round_ratio_ties_even(3_500, 1_000), 4);
        assert_eq!(round_ratio_ties_even(3_499, 1_000), 3);
    }

    #[test]
    fn generation_tile_plan_covers_partial_final_tile() {
        let plan = TilePlan::new(7, 320, 40, 3, 3);
        assert_eq!(plan.input_count, 7);
        assert_eq!(plan.tile_count, 1);
        assert_eq!(plan.thumbnails_per_tile, 9);
        assert_eq!(plan.width, 320);
        assert_eq!(plan.height, 40);
    }

    #[test]
    fn generation_prepares_configured_interval_and_even_width() {
        assert_eq!(even_width(320), 320);
        assert_eq!(even_width(321), 320);
        assert_eq!(even_width(1), 2);
        assert_eq!(format_seconds(10_500), "10.500");
    }
}
