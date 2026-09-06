use std::{fmt::Write as _, sync::Arc};

use jellyfin_data::{
    MediaSegmentRecord, MediaSegmentRepository, MediaSegmentStoreError, NewMediaSegment,
};
use jellyfin_model::{MediaSegmentDto, MediaSegmentType};
use md5::{Digest, Md5};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum MediaSegmentError {
    #[error(transparent)]
    Store(#[from] MediaSegmentStoreError),
}

/// Coordinates media-segment persistence and DTO projection.
#[derive(Clone)]
pub struct MediaSegmentManagerService {
    repository: MediaSegmentRepository,
    provider_names: Arc<Vec<String>>,
}

impl MediaSegmentManagerService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self {
            repository: MediaSegmentRepository::new(database),
            provider_names: Arc::new(Vec::new()),
        }
    }

    /// Replaces the registered media-segment providers in execution order.
    #[must_use]
    pub fn with_provider_names(mut self, provider_names: Vec<String>) -> Self {
        self.provider_names = Arc::new(provider_names);
        self
    }

    /// Returns registered provider names in execution order.
    pub fn provider_names(&self) -> impl Iterator<Item = &str> {
        self.provider_names.iter().map(String::as_str)
    }

    /// Lists persisted segments for one item in official start order.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn list(
        &self,
        item_id: Uuid,
        include_types: &[MediaSegmentType],
        disabled_provider_names: &[String],
    ) -> Result<Vec<MediaSegmentDto>, MediaSegmentError> {
        let include_types = include_types
            .iter()
            .map(|segment_type| *segment_type as i32)
            .collect::<Vec<_>>();
        let provider_ids = self
            .provider_names()
            .filter(|provider_name| {
                !disabled_provider_names
                    .iter()
                    .any(|disabled| disabled.eq_ignore_ascii_case(provider_name))
            })
            .map(media_segment_provider_id)
            .collect::<Vec<_>>();
        Ok(self
            .repository
            .list_for_item_by_providers(
                item_id,
                (!include_types.is_empty()).then_some(include_types.as_slice()),
                &provider_ids,
            )
            .await?
            .into_iter()
            .map(|record| media_segment_dto(&record))
            .collect())
    }

    /// Persists a provider-emitted media segment.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn create(
        &self,
        item_id: Uuid,
        segment_type: MediaSegmentType,
        start_ticks: i64,
        end_ticks: i64,
        segment_provider_id: &str,
    ) -> Result<MediaSegmentDto, MediaSegmentError> {
        let record = self
            .repository
            .create(NewMediaSegment {
                item_id,
                segment_type: segment_type as i32,
                start_ticks,
                end_ticks,
                segment_provider_id: segment_provider_id.to_owned(),
            })
            .await?;
        Ok(media_segment_dto(&record))
    }

    /// Deletes all persisted segments for one item.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete_for_item(&self, item_id: Uuid) -> Result<u64, MediaSegmentError> {
        Ok(self.repository.delete_for_item(item_id).await?)
    }
}

/// Returns the identifier Jellyfin derives from an invariant-lowercase provider name.
#[must_use]
pub fn media_segment_provider_id(name: &str) -> String {
    let utf16le = name
        .to_lowercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut digest: [u8; 16] = Md5::digest(utf16le).into();
    digest[..4].reverse();
    digest[4..6].reverse();
    digest[6..8].reverse();
    let mut provider_id = String::with_capacity(32);
    for byte in digest {
        write!(&mut provider_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    provider_id
}

fn media_segment_dto(record: &MediaSegmentRecord) -> MediaSegmentDto {
    MediaSegmentDto {
        id: record.id,
        item_id: record.item_id,
        segment_type: segment_type_from_code(record.segment_type),
        start_ticks: record.start_ticks,
        end_ticks: record.end_ticks,
    }
}

fn segment_type_from_code(value: i32) -> MediaSegmentType {
    match value {
        1 => MediaSegmentType::Commercial,
        2 => MediaSegmentType::Preview,
        3 => MediaSegmentType::Recap,
        4 => MediaSegmentType::Outro,
        5 => MediaSegmentType::Intro,
        _ => MediaSegmentType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::media_segment_provider_id;

    #[test]
    fn provider_id_matches_official_utf16_guid_bytes() {
        assert_eq!(
            media_segment_provider_id("Test Provider"),
            "95a064062e7f4f6630abfb064256dd5d"
        );
        assert_eq!(
            media_segment_provider_id("TEST PROVIDER"),
            media_segment_provider_id("Test Provider")
        );
    }
}
