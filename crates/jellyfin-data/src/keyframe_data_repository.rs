use sea_orm::{
    ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryOrder, Statement,
    TransactionTrait,
};
use serde_json::{Number, Value};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::keyframe_data;

/// Keyframe information accepted by the persistence layer.
///
/// Values are intentionally not constrained beyond their `i64` representation.
/// Jellyfin's HLS behavior accepts a final keyframe beyond the reported duration
/// and clamps the effective duration when consuming the data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewKeyframeData {
    pub total_duration: i64,
    pub keyframe_ticks: Vec<i64>,
}

/// Strongly typed keyframe row loaded from `PostgreSQL`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyframeDataRecord {
    pub item_id: Uuid,
    pub total_duration: i64,
    pub keyframe_ticks: Vec<i64>,
}

/// Decoded rows and explicitly reported corrupt rows for a backup export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyframeDataExport {
    pub records: Vec<KeyframeDataRecord>,
    pub skipped_item_ids: Vec<Uuid>,
}

/// Keyframe persistence or decoding failure.
#[derive(Debug, Error)]
pub enum KeyframeDataStoreError {
    #[error("base item {item_id} was not found")]
    BaseItemNotFound { item_id: Uuid },
    #[error("keyframe ticks for item {item_id} are not an i64 array")]
    CorruptTicks {
        item_id: Uuid,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed keyframe data storage.
#[derive(Clone)]
pub struct KeyframeDataRepository {
    database: crate::SharedDatabase,
}

impl KeyframeDataRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Inserts or atomically replaces one item's keyframe data.
    ///
    /// `FOR KEY SHARE` keeps the owner alive through the upsert while
    /// `ON CONFLICT` serializes competing writers on the item primary key.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-item error or a database error.
    pub async fn save(
        &self,
        item_id: Uuid,
        data: NewKeyframeData,
    ) -> Result<KeyframeDataRecord, KeyframeDataStoreError> {
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR KEY SHARE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(KeyframeDataStoreError::BaseItemNotFound { item_id });
        }

        let ticks = Value::Array(
            data.keyframe_ticks
                .into_iter()
                .map(|tick| Value::Number(Number::from(tick)))
                .collect(),
        );
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.keyframe_data (
                item_id, total_duration, keyframe_ticks
            )
            VALUES ($1, $2, $3)
            ON CONFLICT (item_id) DO UPDATE
                SET total_duration = EXCLUDED.total_duration,
                    keyframe_ticks = EXCLUDED.keyframe_ticks
            RETURNING item_id, total_duration, keyframe_ticks
            ",
            [item_id.into(), data.total_duration.into(), ticks.into()],
        );
        let row = keyframe_data::Model::find_by_statement(statement)
            .one(&transaction)
            .await?
            .ok_or_else(|| {
                KeyframeDataStoreError::Database(DbErr::RecordNotFound(
                    "keyframe upsert returned no row".to_owned(),
                ))
            })?;
        let record = KeyframeDataRecord::try_from(row)?;
        transaction.commit().await?;
        Ok(record)
    }

    /// Gets one item's keyframe row.
    ///
    /// # Errors
    ///
    /// Returns a typed corruption error when the JSONB array contains values
    /// that cannot be decoded as `i64`, or a database error.
    pub async fn get(
        &self,
        item_id: Uuid,
    ) -> Result<Option<KeyframeDataRecord>, KeyframeDataStoreError> {
        keyframe_data::Entity::find_by_id(item_id)
            .one(self.database.as_ref())
            .await?
            .map(KeyframeDataRecord::try_from)
            .transpose()
    }

    /// Loads all semantically valid rows for backup and reports corrupt rows.
    ///
    /// `PostgreSQL` guarantees that every payload is a JSON array. This method
    /// still decodes each row independently because an array can contain a
    /// non-`i64` value. One bad row therefore cannot abort the healthy export,
    /// and consumers receive every skipped item ID for logging or diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a database error when the ordered scan fails.
    pub async fn export_valid(&self) -> Result<KeyframeDataExport, KeyframeDataStoreError> {
        let rows = keyframe_data::Entity::find()
            .order_by_asc(keyframe_data::Column::ItemId)
            .all(self.database.as_ref())
            .await?;
        let mut records = Vec::with_capacity(rows.len());
        let mut skipped_item_ids = Vec::new();
        for row in rows {
            let item_id = row.item_id;
            match KeyframeDataRecord::try_from(row) {
                Ok(record) => records.push(record),
                Err(KeyframeDataStoreError::CorruptTicks { .. }) => {
                    skipped_item_ids.push(item_id);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(KeyframeDataExport {
            records,
            skipped_item_ids,
        })
    }

    /// Deletes one item's keyframe row, returning whether a row existed.
    ///
    /// Deletion does not decode the tick payload, so callers can remove a
    /// corrupt row after handling its typed read error.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete(&self, item_id: Uuid) -> Result<bool, KeyframeDataStoreError> {
        let result = keyframe_data::Entity::delete_by_id(item_id)
            .exec(self.database.as_ref())
            .await?;
        Ok(result.rows_affected > 0)
    }
}

impl TryFrom<keyframe_data::Model> for KeyframeDataRecord {
    type Error = KeyframeDataStoreError;

    fn try_from(row: keyframe_data::Model) -> Result<Self, Self::Error> {
        let keyframe_ticks = serde_json::from_value(row.keyframe_ticks).map_err(|source| {
            KeyframeDataStoreError::CorruptTicks {
                item_id: row.item_id,
                source,
            }
        })?;
        Ok(Self {
            item_id: row.item_id,
            total_duration: row.total_duration,
            keyframe_ticks,
        })
    }
}
