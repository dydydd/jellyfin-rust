use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use jellyfin_data::{
    NewTrickplayInfo, TrickplayInfo, TrickplayInfoRepository, TrickplayInfoStoreError,
};
use jellyfin_model::TrickplayInfoDto;
use sea_orm::DatabaseConnection;
use thiserror::Error;
use tokio::fs;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum TrickplayError {
    #[error(transparent)]
    Store(#[from] TrickplayInfoStoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads trickplay metadata and maps it to Jellyfin's playlist and tile layout.
#[derive(Clone)]
pub struct TrickplayService {
    repository: TrickplayInfoRepository,
    storage_directory: PathBuf,
}

impl TrickplayService {
    #[must_use]
    pub fn new(database: DatabaseConnection, storage_directory: impl Into<PathBuf>) -> Self {
        Self {
            repository: TrickplayInfoRepository::new(database),
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
}
