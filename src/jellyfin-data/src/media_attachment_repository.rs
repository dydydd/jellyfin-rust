use std::collections::{HashMap, HashSet};

use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::media_attachment;

/// Media-attachment fields stored by Jellyfin.
#[derive(Clone, Debug, PartialEq)]
pub struct PersistedMediaAttachment {
    pub attachment_index: i32,
    pub codec: Option<String>,
    pub codec_tag: Option<String>,
    pub comment: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub delivery_url: Option<String>,
}

/// Item-scoped media-attachment query matching Jellyfin's repository filters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaAttachmentQuery {
    pub item_id: Uuid,
    pub attachment_index: Option<i32>,
}

impl MediaAttachmentQuery {
    #[must_use]
    pub const fn for_item(item_id: Uuid) -> Self {
        Self {
            item_id,
            attachment_index: None,
        }
    }
}

/// Media-attachment persistence or validation failure.
#[derive(Debug, Error)]
pub enum MediaAttachmentStoreError {
    #[error("base item {item_id} was not found")]
    BaseItemNotFound { item_id: Uuid },
    #[error("duplicate media-attachment index {attachment_index}")]
    DuplicateAttachmentIndex { attachment_index: i32 },
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed media-attachment storage.
#[derive(Clone)]
pub struct MediaAttachmentRepository {
    database: crate::SharedDatabase,
}

impl MediaAttachmentRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Atomically replaces every attachment for one item.
    ///
    /// The owner row is locked, stale rows are deleted in set form, and the new
    /// attachment set is expanded with `jsonb_to_recordset` before one upsert.
    ///
    /// # Errors
    ///
    /// Returns duplicate-index, missing-item, or database errors.
    pub async fn replace(
        &self,
        item_id: Uuid,
        attachments: &[PersistedMediaAttachment],
    ) -> Result<Vec<PersistedMediaAttachment>, MediaAttachmentStoreError> {
        validate_unique_indexes(attachments)?;
        let payload = Value::Array(
            attachments
                .iter()
                .map(PersistedMediaAttachment::to_json)
                .collect(),
        );
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(MediaAttachmentStoreError::BaseItemNotFound { item_id });
        }

        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH input AS (
                    SELECT attachment_index
                    FROM jsonb_to_recordset($2::jsonb) AS attachment(attachment_index integer)
                )
                DELETE FROM jellyfin.media_attachments AS stored
                WHERE stored.item_id = $1
                  AND NOT EXISTS (
                      SELECT 1 FROM input
                      WHERE input.attachment_index = stored.attachment_index
                  )
                ",
                [item_id.into(), Value::from(payload.as_str()).into()],
            ))
            .await?;

        let rows = media_attachment::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            UPSERT_SQL,
            [item_id.into(), payload.into()],
        ))
        .all(&transaction)
        .await?;
        let attachments = rows
            .into_iter()
            .map(PersistedMediaAttachment::from)
            .collect();
        transaction.commit().await?;
        Ok(attachments)
    }

    /// Queries one item's attachments, optionally filtered by index.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn query(
        &self,
        query: MediaAttachmentQuery,
    ) -> Result<Vec<PersistedMediaAttachment>, MediaAttachmentStoreError> {
        let mut select = media_attachment::Entity::find()
            .filter(media_attachment::Column::ItemId.eq(query.item_id));
        if let Some(attachment_index) = query.attachment_index {
            select = select.filter(media_attachment::Column::AttachmentIndex.eq(attachment_index));
        }
        let rows = select
            .order_by_asc(media_attachment::Column::AttachmentIndex)
            .all(self.database.as_ref())
            .await?;
        Ok(rows
            .into_iter()
            .map(PersistedMediaAttachment::from)
            .collect())
    }

    /// Queries many items' attachments in one `PostgreSQL` round-trip.
    ///
    /// Attachments are grouped by item id and ordered by attachment index inside
    /// each group so callers can project media sources without extra sorting.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn query_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<PersistedMediaAttachment>>, MediaAttachmentStoreError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = media_attachment::Entity::find()
            .filter(media_attachment::Column::ItemId.is_in(item_ids.iter().copied()))
            .order_by_asc(media_attachment::Column::ItemId)
            .order_by_asc(media_attachment::Column::AttachmentIndex)
            .all(self.database.as_ref())
            .await?;

        let mut grouped = HashMap::with_capacity(item_ids.len());
        for row in rows {
            let item_id = row.item_id;
            grouped
                .entry(item_id)
                .or_insert_with(Vec::new)
                .push(PersistedMediaAttachment::from(row));
        }
        Ok(grouped)
    }
}

fn validate_unique_indexes(
    attachments: &[PersistedMediaAttachment],
) -> Result<(), MediaAttachmentStoreError> {
    let mut indexes = HashSet::with_capacity(attachments.len());
    for attachment in attachments {
        if !indexes.insert(attachment.attachment_index) {
            return Err(MediaAttachmentStoreError::DuplicateAttachmentIndex {
                attachment_index: attachment.attachment_index,
            });
        }
    }
    Ok(())
}

impl PersistedMediaAttachment {
    fn to_json(&self) -> Value {
        json!({
            "attachment_index": self.attachment_index,
            "codec": self.codec,
            "codec_tag": self.codec_tag,
            "comment": self.comment,
            "file_name": self.file_name,
            "mime_type": self.mime_type,
            "delivery_url": self.delivery_url,
        })
    }
}

impl From<media_attachment::Model> for PersistedMediaAttachment {
    fn from(row: media_attachment::Model) -> Self {
        Self {
            attachment_index: row.attachment_index,
            codec: row.codec,
            codec_tag: row.codec_tag,
            comment: row.comment,
            file_name: row.file_name,
            mime_type: row.mime_type,
            delivery_url: row.delivery_url,
        }
    }
}

const UPSERT_SQL: &str = r"
    WITH input AS (
        SELECT *
        FROM jsonb_to_recordset($2::jsonb) AS attachment(
            attachment_index integer, codec text, codec_tag text, comment text,
            file_name text, mime_type text, delivery_url text
        )
    ), upserted AS (
        INSERT INTO jellyfin.media_attachments (
            item_id, attachment_index, codec, codec_tag, comment,
            file_name, mime_type, delivery_url
        )
        SELECT
            $1, attachment_index, codec, codec_tag, comment,
            file_name, mime_type, delivery_url
        FROM input
        ON CONFLICT (item_id, attachment_index) DO UPDATE SET
            (codec, codec_tag, comment, file_name, mime_type, delivery_url) =
            (EXCLUDED.codec, EXCLUDED.codec_tag, EXCLUDED.comment,
             EXCLUDED.file_name, EXCLUDED.mime_type, EXCLUDED.delivery_url)
        RETURNING *
    )
    SELECT * FROM upserted ORDER BY attachment_index
";
