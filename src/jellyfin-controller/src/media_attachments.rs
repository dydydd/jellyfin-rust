use std::collections::HashMap;

use jellyfin_data::{
    MediaAttachmentQuery as PersistedMediaAttachmentQuery, MediaAttachmentRepository,
    MediaAttachmentStoreError, PersistedMediaAttachment,
};
use jellyfin_model::MediaAttachment;
use thiserror::Error;
use uuid::Uuid;

/// Item-scoped media-attachment query in the API model vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaAttachmentFilter {
    pub item_id: Uuid,
    pub index: Option<i32>,
}

impl MediaAttachmentFilter {
    #[must_use]
    pub const fn for_item(item_id: Uuid) -> Self {
        Self {
            item_id,
            index: None,
        }
    }
}

/// Projects persisted media-attachment rows to Jellyfin API DTOs and back.
#[derive(Debug, Default, Clone, Copy)]
pub struct MediaAttachmentMapper;

impl MediaAttachmentMapper {
    #[must_use]
    pub fn to_api(self, persisted: PersistedMediaAttachment) -> MediaAttachment {
        MediaAttachment {
            codec: persisted.codec,
            codec_tag: persisted.codec_tag,
            comment: persisted.comment,
            index: persisted.attachment_index,
            file_name: persisted.file_name,
            mime_type: persisted.mime_type,
            delivery_url: persisted.delivery_url,
        }
    }

    #[must_use]
    pub fn to_persisted(self, attachment: MediaAttachment) -> PersistedMediaAttachment {
        PersistedMediaAttachment {
            attachment_index: attachment.index,
            codec: attachment.codec,
            codec_tag: attachment.codec_tag,
            comment: attachment.comment,
            file_name: attachment.file_name,
            mime_type: attachment.mime_type,
            delivery_url: attachment.delivery_url,
        }
    }
}

/// API-facing service for persisted media-attachment metadata.
#[derive(Clone)]
pub struct MediaAttachmentService {
    repository: MediaAttachmentRepository,
    mapper: MediaAttachmentMapper,
}

impl MediaAttachmentService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self {
            repository: MediaAttachmentRepository::new(database),
            mapper: MediaAttachmentMapper,
        }
    }

    /// Replaces one item's persisted attachments and returns API-projected rows.
    ///
    /// # Errors
    ///
    /// Returns validation or `PostgreSQL` persistence errors.
    pub async fn save_media_attachments(
        &self,
        item_id: Uuid,
        attachments: Vec<MediaAttachment>,
    ) -> Result<Vec<MediaAttachment>, MediaAttachmentServiceError> {
        let persisted = attachments
            .into_iter()
            .map(|attachment| self.mapper.to_persisted(attachment))
            .collect::<Vec<_>>();
        let stored = self.repository.replace(item_id, &persisted).await?;
        Ok(stored
            .into_iter()
            .map(|attachment| self.mapper.to_api(attachment))
            .collect())
    }

    /// Queries persisted media attachments using Jellyfin's item/index filters.
    ///
    /// # Errors
    ///
    /// Returns `PostgreSQL` persistence errors.
    pub async fn get_media_attachments(
        &self,
        filter: MediaAttachmentFilter,
    ) -> Result<Vec<MediaAttachment>, MediaAttachmentServiceError> {
        let query = PersistedMediaAttachmentQuery {
            item_id: filter.item_id,
            attachment_index: filter.index,
        };
        let stored = self.repository.query(query).await?;
        Ok(stored
            .into_iter()
            .map(|attachment| self.mapper.to_api(attachment))
            .collect())
    }

    /// Queries many items' attachments in one database round-trip.
    ///
    /// # Errors
    ///
    /// Returns `PostgreSQL` persistence errors.
    pub async fn get_media_attachments_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<MediaAttachment>>, MediaAttachmentServiceError> {
        let stored = self.repository.query_for_items(item_ids).await?;
        Ok(stored
            .into_iter()
            .map(|(item_id, attachments)| {
                (
                    item_id,
                    attachments
                        .into_iter()
                        .map(|attachment| self.mapper.to_api(attachment))
                        .collect(),
                )
            })
            .collect())
    }
}

#[derive(Debug, Error)]
pub enum MediaAttachmentServiceError {
    #[error(transparent)]
    Store(#[from] MediaAttachmentStoreError),
}
