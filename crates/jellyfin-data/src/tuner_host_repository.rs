use sea_orm::{
    DbBackend, DbErr, EntityTrait, FromQueryResult, QueryOrder, Statement, TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::tuner_host;

/// Complete tuner-host configuration written by the Live TV service.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct NewTunerHost {
    pub requested_id: Option<Uuid>,
    pub url: String,
    pub tuner_type: String,
    pub device_id: Option<String>,
    pub friendly_name: Option<String>,
    pub import_favorites_only: bool,
    pub allow_hw_transcoding: bool,
    pub allow_fmp4_transcoding_container: bool,
    pub allow_stream_sharing: bool,
    pub fallback_max_streaming_bitrate: i32,
    pub enable_stream_looping: bool,
    pub source: Option<String>,
    pub tuner_count: i32,
    pub user_agent: Option<String>,
    pub ignore_dts: bool,
    pub read_at_native_framerate: bool,
}

#[derive(Debug, Error)]
pub enum TunerHostStoreError {
    #[error("tuner count and fallback bitrate must be nonnegative")]
    InvalidNumericValue,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed tuner-host configuration repository.
#[derive(Clone)]
pub struct TunerHostRepository {
    database: crate::SharedDatabase,
}

impl TunerHostRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Inserts a new host or atomically updates an existing requested ID.
    ///
    /// A nonexistent requested ID intentionally resolves to a fresh UUID,
    /// matching Jellyfin's configuration-manager behavior. The resolution and
    /// upsert execute as one `PostgreSQL` statement inside one transaction.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn save(&self, host: NewTunerHost) -> Result<tuner_host::Model, TunerHostStoreError> {
        if host.tuner_count < 0 || host.fallback_max_streaming_bitrate < 0 {
            return Err(TunerHostStoreError::InvalidNumericValue);
        }
        let transaction = self.database.begin().await?;
        let generated_id = Uuid::new_v4();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH resolved AS (
                SELECT COALESCE(
                    (SELECT id FROM jellyfin.tuner_hosts WHERE id = $1::uuid),
                    $2::uuid
                ) AS id
            )
            INSERT INTO jellyfin.tuner_hosts (
                id, url, tuner_type, device_id, friendly_name,
                import_favorites_only, allow_hw_transcoding,
                allow_fmp4_transcoding_container, allow_stream_sharing,
                fallback_max_streaming_bitrate, enable_stream_looping,
                source, tuner_count, user_agent, ignore_dts,
                read_at_native_framerate
            )
            SELECT id, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                   $13, $14, $15, $16, $17
            FROM resolved
            ON CONFLICT (id) DO UPDATE SET
                url = EXCLUDED.url,
                tuner_type = EXCLUDED.tuner_type,
                device_id = EXCLUDED.device_id,
                friendly_name = EXCLUDED.friendly_name,
                import_favorites_only = EXCLUDED.import_favorites_only,
                allow_hw_transcoding = EXCLUDED.allow_hw_transcoding,
                allow_fmp4_transcoding_container = EXCLUDED.allow_fmp4_transcoding_container,
                allow_stream_sharing = EXCLUDED.allow_stream_sharing,
                fallback_max_streaming_bitrate = EXCLUDED.fallback_max_streaming_bitrate,
                enable_stream_looping = EXCLUDED.enable_stream_looping,
                source = EXCLUDED.source,
                tuner_count = EXCLUDED.tuner_count,
                user_agent = EXCLUDED.user_agent,
                ignore_dts = EXCLUDED.ignore_dts,
                read_at_native_framerate = EXCLUDED.read_at_native_framerate
            RETURNING id, url, tuner_type, device_id, friendly_name,
                import_favorites_only, allow_hw_transcoding,
                allow_fmp4_transcoding_container, allow_stream_sharing,
                fallback_max_streaming_bitrate, enable_stream_looping,
                source, tuner_count, user_agent, ignore_dts,
                read_at_native_framerate, date_created, date_modified, row_version
            ",
            [
                host.requested_id.into(),
                generated_id.into(),
                host.url.into(),
                host.tuner_type.into(),
                host.device_id.into(),
                host.friendly_name.into(),
                host.import_favorites_only.into(),
                host.allow_hw_transcoding.into(),
                host.allow_fmp4_transcoding_container.into(),
                host.allow_stream_sharing.into(),
                host.fallback_max_streaming_bitrate.into(),
                host.enable_stream_looping.into(),
                host.source.into(),
                host.tuner_count.into(),
                host.user_agent.into(),
                host.ignore_dts.into(),
                host.read_at_native_framerate.into(),
            ],
        );
        let saved = tuner_host::Model::find_by_statement(statement)
            .one(&transaction)
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("tuner host upsert returned no row".to_owned()))?;
        transaction.commit().await?;
        Ok(saved)
    }

    /// Returns every configured host in stable ID order.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn list(&self) -> Result<Vec<tuner_host::Model>, TunerHostStoreError> {
        Ok(tuner_host::Entity::find()
            .order_by_asc(tuner_host::Column::Id)
            .all(self.database.as_ref())
            .await?)
    }

    /// Deletes an exact ID. A missing ID is an intentional no-op.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete(&self, id: Option<Uuid>) -> Result<bool, TunerHostStoreError> {
        let Some(id) = id else {
            return Ok(false);
        };
        Ok(tuner_host::Entity::delete_by_id(id)
            .exec(self.database.as_ref())
            .await?
            .rows_affected
            == 1)
    }
}
