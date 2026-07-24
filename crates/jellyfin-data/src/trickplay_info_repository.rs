use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, FromQueryResult, Statement,
    TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::trickplay_info;

/// Metadata for one trickplay thumbnail resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrickplayInfo {
    pub item_id: Uuid,
    pub width: i32,
    pub height: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub thumbnail_count: i32,
    pub interval: i32,
    pub bandwidth: i32,
}

/// Values used to create or replace a trickplay resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewTrickplayInfo {
    pub width: i32,
    pub height: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub thumbnail_count: i32,
    pub interval: i32,
    pub bandwidth: i32,
}

#[derive(Debug, Error)]
pub enum TrickplayInfoStoreError {
    #[error("base item {item_id} was not found")]
    BaseItemNotFound { item_id: Uuid },
    #[error("trickplay field {field} must be {requirement}")]
    InvalidValue {
        field: &'static str,
        requirement: &'static str,
    },
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct TrickplayInfoRepository {
    database: DatabaseConnection,
}

impl TrickplayInfoRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Loads metadata for one item and thumbnail width.
    ///
    /// # Errors
    ///
    /// Returns a database error when `PostgreSQL` cannot execute the query.
    pub async fn get(
        &self,
        item_id: Uuid,
        width: i32,
    ) -> Result<Option<TrickplayInfo>, TrickplayInfoStoreError> {
        Ok(trickplay_info::Entity::find_by_id((item_id, width))
            .one(&self.database)
            .await?
            .map(TrickplayInfo::from))
    }

    /// Atomically creates or replaces metadata for one resolution.
    ///
    /// `PostgreSQL`'s composite-key conflict handler makes concurrent writers
    /// converge without a read-before-write race.
    ///
    /// # Errors
    ///
    /// Returns validation, missing-owner, or database errors.
    pub async fn upsert(
        &self,
        item_id: Uuid,
        info: NewTrickplayInfo,
    ) -> Result<TrickplayInfo, TrickplayInfoStoreError> {
        info.validate()?;
        let transaction = self.database.begin().await?;
        if transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR KEY SHARE",
                [item_id.into()],
            ))
            .await?
            .is_none()
        {
            return Err(TrickplayInfoStoreError::BaseItemNotFound { item_id });
        }

        let values = [
            item_id.into(),
            info.width.into(),
            info.height.into(),
            info.tile_width.into(),
            info.tile_height.into(),
            info.thumbnail_count.into(),
            info.interval.into(),
            info.bandwidth.into(),
        ];
        let row = trickplay_info::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.trickplay_infos (
                item_id, width, height, tile_width, tile_height,
                thumbnail_count, interval, bandwidth
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (item_id, width) DO UPDATE SET
                height = EXCLUDED.height,
                tile_width = EXCLUDED.tile_width,
                tile_height = EXCLUDED.tile_height,
                thumbnail_count = EXCLUDED.thumbnail_count,
                interval = EXCLUDED.interval,
                bandwidth = EXCLUDED.bandwidth
            RETURNING *
            ",
            values,
        ))
        .one(&transaction)
        .await?
        .ok_or_else(|| DbErr::Custom("trickplay upsert returned no row".to_owned()))?;
        transaction.commit().await?;
        Ok(row.into())
    }
}

impl NewTrickplayInfo {
    fn validate(self) -> Result<(), TrickplayInfoStoreError> {
        for (field, value) in [
            ("width", self.width),
            ("height", self.height),
            ("tile_width", self.tile_width),
            ("tile_height", self.tile_height),
            ("interval", self.interval),
        ] {
            if value <= 0 {
                return Err(TrickplayInfoStoreError::InvalidValue {
                    field,
                    requirement: "positive",
                });
            }
        }
        for (field, value) in [
            ("thumbnail_count", self.thumbnail_count),
            ("bandwidth", self.bandwidth),
        ] {
            if value < 0 {
                return Err(TrickplayInfoStoreError::InvalidValue {
                    field,
                    requirement: "nonnegative",
                });
            }
        }
        Ok(())
    }
}

impl From<trickplay_info::Model> for TrickplayInfo {
    fn from(row: trickplay_info::Model) -> Self {
        Self {
            item_id: row.item_id,
            width: row.width,
            height: row.height,
            tile_width: row.tile_width,
            tile_height: row.tile_height,
            thumbnail_count: row.thumbnail_count,
            interval: row.interval,
            bandwidth: row.bandwidth,
        }
    }
}
