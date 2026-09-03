use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::media_segment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMediaSegment {
    pub item_id: Uuid,
    pub segment_type: i32,
    pub start_ticks: i64,
    pub end_ticks: i64,
    pub segment_provider_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSegmentRecord {
    pub id: Uuid,
    pub item_id: Uuid,
    pub segment_type: i32,
    pub start_ticks: i64,
    pub end_ticks: i64,
    pub segment_provider_id: String,
}

#[derive(Debug, Error)]
pub enum MediaSegmentStoreError {
    #[error("media segment {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("media segment provider id exceeds its 64 character limit")]
    ProviderTooLong,
    #[error("media segment type must be between 0 and 5")]
    InvalidType,
    #[error("media segment end must not be before its start")]
    InvalidRange,
    #[error("media segment provider id does not match the replacement scope")]
    ProviderMismatch,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed persistence for Jellyfin media segments.
#[derive(Clone)]
pub struct MediaSegmentRepository {
    database: crate::SharedDatabase,
}

impl MediaSegmentRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Inserts one media segment.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn create(
        &self,
        segment: NewMediaSegment,
    ) -> Result<MediaSegmentRecord, MediaSegmentStoreError> {
        validate_segment(&segment)?;
        let model = media_segment::ActiveModel {
            id: Set(Uuid::new_v4()),
            item_id: Set(segment.item_id),
            segment_type: Set(segment.segment_type),
            start_ticks: Set(segment.start_ticks),
            end_ticks: Set(segment.end_ticks),
            segment_provider_id: Set(segment.segment_provider_id),
        }
        .insert(self.database.as_ref())
        .await?;
        Ok(model.into())
    }

    /// Replaces every segment emitted by one provider for an item.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn replace_for_provider(
        &self,
        item_id: Uuid,
        segment_provider_id: &str,
        segments: Vec<NewMediaSegment>,
    ) -> Result<Vec<MediaSegmentRecord>, MediaSegmentStoreError> {
        for segment in &segments {
            validate_segment(segment)?;
            if segment.segment_provider_id != segment_provider_id {
                return Err(MediaSegmentStoreError::ProviderMismatch);
            }
        }
        let transaction = self.database.begin().await?;
        media_segment::Entity::delete_many()
            .filter(media_segment::Column::ItemId.eq(item_id))
            .filter(media_segment::Column::SegmentProviderId.eq(segment_provider_id))
            .exec(&transaction)
            .await?;
        let mut records = Vec::with_capacity(segments.len());
        for segment in segments {
            let model = media_segment::ActiveModel {
                id: Set(Uuid::new_v4()),
                item_id: Set(segment.item_id),
                segment_type: Set(segment.segment_type),
                start_ticks: Set(segment.start_ticks),
                end_ticks: Set(segment.end_ticks),
                segment_provider_id: Set(segment.segment_provider_id),
            }
            .insert(&transaction)
            .await?;
            records.push(model.into());
        }
        transaction.commit().await?;
        Ok(records)
    }

    /// Lists segments for one item, optionally filtered by type, in start order.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn list_for_item(
        &self,
        item_id: Uuid,
        include_types: Option<&[i32]>,
    ) -> Result<Vec<MediaSegmentRecord>, MediaSegmentStoreError> {
        let mut query =
            media_segment::Entity::find().filter(media_segment::Column::ItemId.eq(item_id));
        if let Some(include_types) = include_types.filter(|types| !types.is_empty()) {
            query = query
                .filter(media_segment::Column::SegmentType.is_in(include_types.iter().copied()));
        }
        Ok(query
            .order_by_asc(media_segment::Column::StartTicks)
            .order_by_asc(media_segment::Column::Id)
            .all(self.database.as_ref())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Deletes all segments for one item.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete_for_item(&self, item_id: Uuid) -> Result<u64, MediaSegmentStoreError> {
        Ok(media_segment::Entity::delete_many()
            .filter(media_segment::Column::ItemId.eq(item_id))
            .exec(self.database.as_ref())
            .await?
            .rows_affected)
    }

    /// Returns whether an item has any persisted segments.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn has_for_item(&self, item_id: Uuid) -> Result<bool, MediaSegmentStoreError> {
        Ok(media_segment::Entity::find()
            .filter(media_segment::Column::ItemId.eq(item_id))
            .one(self.database.as_ref())
            .await?
            .is_some())
    }
}

impl From<media_segment::Model> for MediaSegmentRecord {
    fn from(model: media_segment::Model) -> Self {
        Self {
            id: model.id,
            item_id: model.item_id,
            segment_type: model.segment_type,
            start_ticks: model.start_ticks,
            end_ticks: model.end_ticks,
            segment_provider_id: model.segment_provider_id,
        }
    }
}

fn validate_segment(segment: &NewMediaSegment) -> Result<(), MediaSegmentStoreError> {
    if segment.segment_provider_id.trim().is_empty() {
        return Err(MediaSegmentStoreError::EmptyField("segment_provider_id"));
    }
    if segment.segment_provider_id.chars().count() > 64 {
        return Err(MediaSegmentStoreError::ProviderTooLong);
    }
    if !(0..=5).contains(&segment.segment_type) {
        return Err(MediaSegmentStoreError::InvalidType);
    }
    if segment.start_ticks < 0 || segment.end_ticks < segment.start_ticks {
        return Err(MediaSegmentStoreError::InvalidRange);
    }
    Ok(())
}
