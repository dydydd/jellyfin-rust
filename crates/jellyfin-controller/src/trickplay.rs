use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use jellyfin_data::{TrickplayInfo, TrickplayInfoRepository, TrickplayInfoStoreError};
use jellyfin_model::TrickplayInfoDto;
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum TrickplayError {
    #[error(transparent)]
    Store(#[from] TrickplayInfoStoreError),
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
    let id = info.item_id.hyphenated().to_string();
    root.join(&id[..2])
        .join(id)
        .join(format!(
            "{} - {}x{}",
            info.width, info.tile_width, info.tile_height
        ))
        .join(format!("{index}.jpg"))
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
}
