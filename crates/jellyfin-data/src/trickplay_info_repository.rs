use std::collections::{BTreeMap, HashMap};

use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, Statement, TransactionTrait, Value, sea_query::OnConflict,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::trickplay_info;

pub type TrickplayManifestStore = BTreeMap<Uuid, BTreeMap<i32, TrickplayInfo>>;
pub type TrickplayManifestStores = HashMap<Uuid, TrickplayManifestStore>;

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
    database: crate::SharedDatabase,
}

impl TrickplayInfoRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
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
            .one(self.database.as_ref())
            .await?
            .map(TrickplayInfo::from))
    }

    /// Loads the local media-source manifests for multiple displayed items.
    ///
    /// One `PostgreSQL` query expands primary-version and linked alternate
    /// relationships, then joins every stored resolution. The outer map is
    /// initialized for every requested item so callers can distinguish an
    /// explicitly requested empty manifest without issuing follow-up queries.
    ///
    /// # Errors
    ///
    /// Returns a database error when the batch query cannot be executed.
    pub async fn manifests_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<TrickplayManifestStores, TrickplayInfoStoreError> {
        let item_ids = unique_ids(item_ids);
        let mut manifests = item_ids
            .iter()
            .copied()
            .map(|item_id| (item_id, BTreeMap::new()))
            .collect::<HashMap<_, _>>();
        if item_ids.is_empty() {
            return Ok(manifests);
        }

        let values = item_ids
            .iter()
            .copied()
            .map(Value::from)
            .collect::<Vec<_>>();
        let placeholders = (1..=values.len())
            .map(|index| format!("(${index}::uuid)"))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = TrickplayManifestRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "WITH RECURSIVE requested(display_item_id) AS (VALUES {placeholders}), \
                 version_roots AS MATERIALIZED (\
                     SELECT requested.display_item_id, \
                            COALESCE(item.primary_version_id, item.id) AS root_id \
                     FROM requested \
                     INNER JOIN jellyfin.base_items AS item \
                       ON item.id = requested.display_item_id \
                     WHERE item.item_type IN \
                           ('Video', 'Movie', 'Episode', 'MusicVideo', 'Trailer')\
                 ), media_sources(display_item_id, source_id) AS (\
                     SELECT display_item_id, root_id FROM version_roots \
                     UNION \
                     SELECT roots.display_item_id, item.id \
                     FROM version_roots AS roots \
                     INNER JOIN jellyfin.base_items AS item \
                       ON item.primary_version_id = roots.root_id \
                     UNION \
                     SELECT sources.display_item_id, \
                            CASE WHEN links.parent_id = sources.source_id \
                                 THEN links.child_id ELSE links.parent_id END \
                     FROM media_sources AS sources \
                     INNER JOIN jellyfin.linked_children AS links \
                       ON (links.parent_id = sources.source_id \
                           OR links.child_id = sources.source_id) \
                      AND links.child_type IN (2, 3)\
                 ) \
                 SELECT sources.display_item_id, info.* \
                 FROM media_sources AS sources \
                 INNER JOIN jellyfin.trickplay_infos AS info \
                   ON info.item_id = sources.source_id \
                 ORDER BY sources.display_item_id, info.item_id, info.width"
            ),
            values,
        ))
        .all(self.database.as_ref())
        .await?;

        for row in rows {
            manifests
                .entry(row.display_item_id)
                .or_default()
                .entry(row.item_id)
                .or_default()
                .insert(row.width, row.into());
        }
        Ok(manifests)
    }

    /// Deletes every stored resolution for one item.
    ///
    /// The item itself is intentionally not locked: deleting absent metadata is
    /// idempotent, and the owner foreign key serializes this operation safely
    /// against concurrent item deletion.
    ///
    /// # Errors
    ///
    /// Returns a database error when the delete cannot be executed.
    pub async fn delete_for_item(&self, item_id: Uuid) -> Result<bool, TrickplayInfoStoreError> {
        let result = trickplay_info::Entity::delete_many()
            .filter(trickplay_info::Column::ItemId.eq(item_id))
            .exec(self.database.as_ref())
            .await?;
        Ok(result.rows_affected > 0)
    }

    /// Reconciles metadata with resolutions discovered in managed storage.
    ///
    /// `present_widths` includes every valid directory that still contains a
    /// JPEG, even when its image header could not be inspected. Such rows are
    /// retained, while `discovered` rows fill only missing primary keys. A row
    /// lock serializes the fixed-size delete/insert/read sequence with item
    /// deletion and competing discovery passes.
    ///
    /// # Errors
    ///
    /// Returns validation, missing-owner, or database errors.
    pub async fn synchronize_discovered(
        &self,
        item_id: Uuid,
        present_widths: &[i32],
        discovered: &[NewTrickplayInfo],
    ) -> Result<Vec<TrickplayInfo>, TrickplayInfoStoreError> {
        let present_widths = unique_widths(present_widths);
        let mut discovered = discovered
            .iter()
            .copied()
            .map(|info| {
                info.validate()?;
                Ok((info.width, info))
            })
            .collect::<Result<BTreeMap<_, _>, TrickplayInfoStoreError>>()?;
        discovered.retain(|width, _| present_widths.binary_search(width).is_ok());

        let transaction = self.database.begin().await?;
        if transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?
            .is_none()
        {
            return Err(TrickplayInfoStoreError::BaseItemNotFound { item_id });
        }

        let mut delete = trickplay_info::Entity::delete_many()
            .filter(trickplay_info::Column::ItemId.eq(item_id));
        if !present_widths.is_empty() {
            delete = delete.filter(trickplay_info::Column::Width.is_not_in(present_widths));
        }
        delete.exec(&transaction).await?;

        if !discovered.is_empty() {
            let rows = discovered
                .into_values()
                .map(|info| trickplay_info::ActiveModel {
                    item_id: Set(item_id),
                    width: Set(info.width),
                    height: Set(info.height),
                    tile_width: Set(info.tile_width),
                    tile_height: Set(info.tile_height),
                    thumbnail_count: Set(info.thumbnail_count),
                    interval: Set(info.interval),
                    bandwidth: Set(info.bandwidth),
                });
            trickplay_info::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([
                        trickplay_info::Column::ItemId,
                        trickplay_info::Column::Width,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .exec_without_returning(&transaction)
                .await?;
        }

        let rows = trickplay_info::Entity::find()
            .filter(trickplay_info::Column::ItemId.eq(item_id))
            .order_by_asc(trickplay_info::Column::Width)
            .all(&transaction)
            .await?
            .into_iter()
            .map(TrickplayInfo::from)
            .collect();
        transaction.commit().await?;
        Ok(rows)
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

#[derive(Debug, FromQueryResult)]
struct TrickplayManifestRow {
    display_item_id: Uuid,
    item_id: Uuid,
    width: i32,
    height: i32,
    tile_width: i32,
    tile_height: i32,
    thumbnail_count: i32,
    interval: i32,
    bandwidth: i32,
}

impl From<TrickplayManifestRow> for TrickplayInfo {
    fn from(row: TrickplayManifestRow) -> Self {
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

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn unique_widths(widths: &[i32]) -> Vec<i32> {
    let mut widths = widths
        .iter()
        .copied()
        .filter(|width| *width > 0)
        .collect::<Vec<_>>();
    widths.sort_unstable();
    widths.dedup();
    widths
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
