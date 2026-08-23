use jellyfin_data::{
    MediaSegmentRecord, MediaSegmentRepository, MediaSegmentStoreError, NewMediaSegment,
};
use jellyfin_model::{MediaSegmentDto, MediaSegmentType};
use sea_orm::DatabaseConnection;
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
}

impl MediaSegmentManagerService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            repository: MediaSegmentRepository::new(database),
        }
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
    ) -> Result<Vec<MediaSegmentDto>, MediaSegmentError> {
        let include_types = include_types
            .iter()
            .map(|segment_type| *segment_type as i32)
            .collect::<Vec<_>>();
        Ok(self
            .repository
            .list_for_item(item_id, (!include_types.is_empty()).then_some(include_types.as_slice()))
            .await?
            .into_iter()
            .map(media_segment_dto)
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
        Ok(media_segment_dto(record))
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

fn media_segment_dto(record: MediaSegmentRecord) -> MediaSegmentDto {
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
