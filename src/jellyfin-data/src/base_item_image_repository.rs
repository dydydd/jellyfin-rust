use std::borrow::Cow;
use std::collections::HashSet;

use chrono::{DateTime, SecondsFormat, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::base_item_image;

/// Stable database representation of Jellyfin's `ImageType` values.
///
/// The numeric values intentionally match Jellyfin's public enum without
/// introducing a dependency from the persistence crate back to the API model.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i16)]
pub enum BaseItemImageType {
    Primary = 0,
    Art = 1,
    Backdrop = 2,
    Banner = 3,
    Logo = 4,
    Thumb = 5,
    Disc = 6,
    Box = 7,
    Screenshot = 8,
    Menu = 9,
    Chapter = 10,
    BoxRear = 11,
    Profile = 12,
}

impl BaseItemImageType {
    pub const ALL: [Self; 13] = [
        Self::Primary,
        Self::Art,
        Self::Backdrop,
        Self::Banner,
        Self::Logo,
        Self::Thumb,
        Self::Disc,
        Self::Box,
        Self::Screenshot,
        Self::Menu,
        Self::Chapter,
        Self::BoxRear,
        Self::Profile,
    ];

    #[must_use]
    pub const fn as_i16(self) -> i16 {
        self as i16
    }
}

impl TryFrom<i16> for BaseItemImageType {
    type Error = InvalidBaseItemImageType;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Primary),
            1 => Ok(Self::Art),
            2 => Ok(Self::Backdrop),
            3 => Ok(Self::Banner),
            4 => Ok(Self::Logo),
            5 => Ok(Self::Thumb),
            6 => Ok(Self::Disc),
            7 => Ok(Self::Box),
            8 => Ok(Self::Screenshot),
            9 => Ok(Self::Menu),
            10 => Ok(Self::Chapter),
            11 => Ok(Self::BoxRear),
            12 => Ok(Self::Profile),
            _ => Err(InvalidBaseItemImageType(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("unknown base-item image type {0}")]
pub struct InvalidBaseItemImageType(pub i16);

/// Image metadata accepted by an atomic item-image replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewBaseItemImage {
    pub image_type: BaseItemImageType,
    pub image_index: u32,
    pub path: String,
    pub date_modified: DateTime<Utc>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub blurhash: Option<String>,
}

/// Persisted, strongly typed base-item image metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseItemImage {
    pub item_id: Uuid,
    pub image_type: BaseItemImageType,
    pub image_index: u32,
    pub path: String,
    pub date_modified: DateTime<Utc>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub blurhash: Option<String>,
}

/// Result of atomically setting a single image or appending a backdrop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredImageMutation {
    pub current: BaseItemImage,
    pub replaced: Option<BaseItemImage>,
}

/// Locked pair of images selected by their public ordinals.
pub struct BaseItemImageSwap {
    pub first: BaseItemImage,
    pub second: BaseItemImage,
    transaction: DatabaseTransaction,
}

impl BaseItemImageSwap {
    /// Refreshes metadata for the two fixed paths and commits the item-row lock.
    ///
    /// # Errors
    ///
    /// Returns a database error when either update or the commit fails.
    pub async fn commit(
        self,
        first_modified: DateTime<Utc>,
        second_modified: DateTime<Utc>,
    ) -> Result<(), BaseItemImageStoreError> {
        for (image, modified) in [
            (&self.first, first_modified),
            (&self.second, second_modified),
        ] {
            self.transaction
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r"
                    UPDATE jellyfin.base_item_images
                    SET date_modified = $4, width = NULL, height = NULL
                    WHERE item_id = $1 AND image_type = $2 AND image_index = $3
                    ",
                    [
                        image.item_id.into(),
                        image.image_type.as_i16().into(),
                        i32::try_from(image.image_index)
                            .map_err(|_| BaseItemImageStoreError::ImageIndexOutOfRange {
                                value: image.image_index,
                            })?
                            .into(),
                        modified.into(),
                    ],
                ))
                .await?;
        }
        self.transaction.commit().await?;
        Ok(())
    }
}

/// Base-item image persistence or input validation failure.
#[derive(Debug, Error)]
pub enum BaseItemImageStoreError {
    #[error("base item {item_id} was not found")]
    BaseItemNotFound { item_id: Uuid },
    #[error("base-item image path cannot be blank")]
    EmptyPath,
    #[error("duplicate base-item image key ({image_type:?}, {image_index})")]
    DuplicateImage {
        image_type: BaseItemImageType,
        image_index: u32,
    },
    #[error("base-item image index {value} exceeds PostgreSQL integer range")]
    ImageIndexOutOfRange { value: u32 },
    #[error("base-item image {field} must be positive and fit PostgreSQL integer range: {value}")]
    InvalidDimension { field: &'static str, value: u32 },
    #[error("{image_type:?} images cannot be uploaded through the item image endpoint")]
    UnsupportedUploadImageType { image_type: BaseItemImageType },
    #[error("{image_type:?} images do not support index changes")]
    UnsupportedSwapImageType { image_type: BaseItemImageType },
    #[error(transparent)]
    InvalidImageType(#[from] InvalidBaseItemImageType),
    #[error("invalid persisted base-item image {field}: {value}")]
    CorruptRow { field: &'static str, value: i32 },
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed base-item image storage.
#[derive(Clone)]
pub struct BaseItemImageRepository {
    database: crate::SharedDatabase,
}

impl BaseItemImageRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Lists one item's images in stable type/index order.
    ///
    /// # Errors
    ///
    /// Returns a database error or a corrupt-row error.
    pub async fn list(&self, item_id: Uuid) -> Result<Vec<BaseItemImage>, BaseItemImageStoreError> {
        let rows = base_item_image::Entity::find()
            .filter(base_item_image::Column::ItemId.eq(item_id))
            .order_by_asc(base_item_image::Column::ImageType)
            .order_by_asc(base_item_image::Column::ImageIndex)
            .all(self.database.as_ref())
            .await?;
        rows.into_iter().map(BaseItemImage::try_from).collect()
    }

    /// Lists images for several items with one query, ordered by item/type/index.
    ///
    /// # Errors
    ///
    /// Returns a database error or a corrupt-row error.
    pub async fn list_many(
        &self,
        item_ids: &[Uuid],
    ) -> Result<Vec<BaseItemImage>, BaseItemImageStoreError> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = base_item_image::Entity::find()
            .filter(base_item_image::Column::ItemId.is_in(item_ids.iter().copied()))
            .order_by_asc(base_item_image::Column::ItemId)
            .order_by_asc(base_item_image::Column::ImageType)
            .order_by_asc(base_item_image::Column::ImageIndex)
            .all(self.database.as_ref())
            .await?;
        rows.into_iter().map(BaseItemImage::try_from).collect()
    }

    /// Loads the primary image through `PostgreSQL`'s partial covering index.
    ///
    /// # Errors
    ///
    /// Returns a database error or a corrupt-row error.
    pub async fn primary(
        &self,
        item_id: Uuid,
    ) -> Result<Option<BaseItemImage>, BaseItemImageStoreError> {
        base_item_image::Entity::find()
            .filter(base_item_image::Column::ItemId.eq(item_id))
            .filter(base_item_image::Column::ImageType.eq(BaseItemImageType::Primary.as_i16()))
            .filter(base_item_image::Column::ImageIndex.eq(0))
            .one(self.database.as_ref())
            .await?
            .map(BaseItemImage::try_from)
            .transpose()
    }

    /// Loads one image through the composite `PostgreSQL` primary key.
    ///
    /// # Errors
    ///
    /// Returns validation, database, or corrupt-row errors.
    pub async fn get(
        &self,
        item_id: Uuid,
        image_type: BaseItemImageType,
        image_index: u32,
    ) -> Result<Option<BaseItemImage>, BaseItemImageStoreError> {
        let image_index = i32::try_from(image_index)
            .map_err(|_| BaseItemImageStoreError::ImageIndexOutOfRange { value: image_index })?;
        base_item_image::Entity::find_by_id((item_id, image_type.as_i16(), image_index))
            .one(self.database.as_ref())
            .await?
            .map(BaseItemImage::try_from)
            .transpose()
    }

    /// Loads an image by Jellyfin's zero-based ordinal within its type.
    ///
    /// Persisted image indexes can contain gaps after metadata refreshes. The
    /// public API nevertheless addresses images by their ordered position, so
    /// `PostgreSQL` resolves that position through the composite primary key.
    ///
    /// # Errors
    ///
    /// Returns database or corrupt-row errors.
    pub async fn at(
        &self,
        item_id: Uuid,
        image_type: BaseItemImageType,
        ordinal: u64,
    ) -> Result<Option<BaseItemImage>, BaseItemImageStoreError> {
        base_item_image::Entity::find()
            .filter(base_item_image::Column::ItemId.eq(item_id))
            .filter(base_item_image::Column::ImageType.eq(image_type.as_i16()))
            .order_by_asc(base_item_image::Column::ImageIndex)
            .offset(ordinal)
            .limit(1)
            .one(self.database.as_ref())
            .await?
            .map(BaseItemImage::try_from)
            .transpose()
    }

    /// Atomically deletes an image by its public zero-based ordinal.
    ///
    /// The owning item row serializes this operation with image replacements.
    /// Persisted indexes are intentionally left unchanged; public ordinals are
    /// derived from their ordered values and therefore close gaps naturally.
    ///
    /// # Errors
    ///
    /// Returns a missing-item, database, or corrupt-row error.
    pub async fn delete_at(
        &self,
        item_id: Uuid,
        image_type: BaseItemImageType,
        ordinal: u64,
    ) -> Result<Option<BaseItemImage>, BaseItemImageStoreError> {
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(BaseItemImageStoreError::BaseItemNotFound { item_id });
        }
        let Some(ordinal) = i64::try_from(ordinal).ok() else {
            transaction.commit().await?;
            return Ok(None);
        };

        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH target AS (
                SELECT image_index
                FROM jellyfin.base_item_images
                WHERE item_id = $1 AND image_type = $2
                ORDER BY image_index
                OFFSET $3
                LIMIT 1
            )
            DELETE FROM jellyfin.base_item_images AS stored
            USING target
            WHERE stored.item_id = $1
              AND stored.image_type = $2
              AND stored.image_index = target.image_index
            RETURNING stored.item_id, stored.image_type, stored.image_index,
                      stored.path, stored.date_modified, stored.width,
                      stored.height, stored.blurhash
            ",
            [item_id.into(), image_type.as_i16().into(), ordinal.into()],
        );
        let deleted = base_item_image::Model::find_by_statement(statement)
            .one(&transaction)
            .await?
            .map(BaseItemImage::try_from)
            .transpose()?;
        transaction.commit().await?;
        Ok(deleted)
    }

    /// Locks and selects two images by public ordinal for a file-content swap.
    ///
    /// A missing ordinal is an intentional no-op. The returned guard retains
    /// the owning item row lock until [`BaseItemImageSwap::commit`] or drop.
    ///
    /// # Errors
    ///
    /// Returns unsupported-type, missing-item, corrupt-row, or database errors.
    pub async fn begin_swap(
        &self,
        item_id: Uuid,
        image_type: BaseItemImageType,
        first_ordinal: i64,
        second_ordinal: i64,
    ) -> Result<Option<BaseItemImageSwap>, BaseItemImageStoreError> {
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(BaseItemImageStoreError::BaseItemNotFound { item_id });
        }
        if !matches!(
            image_type,
            BaseItemImageType::Backdrop | BaseItemImageType::Chapter
        ) {
            return Err(BaseItemImageStoreError::UnsupportedSwapImageType { image_type });
        }
        if first_ordinal < 0 {
            transaction.commit().await?;
            return Ok(None);
        }
        if second_ordinal < 0 {
            transaction.commit().await?;
            return Ok(None);
        }
        let select = |ordinal: i64| {
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                SELECT item_id, image_type, image_index, path, date_modified,
                       width, height, blurhash
                FROM jellyfin.base_item_images
                WHERE item_id = $1 AND image_type = $2
                ORDER BY image_index
                OFFSET $3 LIMIT 1
                ",
                [item_id.into(), image_type.as_i16().into(), ordinal.into()],
            )
        };
        let first = base_item_image::Model::find_by_statement(select(first_ordinal))
            .one(&transaction)
            .await?
            .map(BaseItemImage::try_from)
            .transpose()?;
        let second = base_item_image::Model::find_by_statement(select(second_ordinal))
            .one(&transaction)
            .await?
            .map(BaseItemImage::try_from)
            .transpose()?;
        let (Some(first), Some(second)) = (first, second) else {
            transaction.commit().await?;
            return Ok(None);
        };
        Ok(Some(BaseItemImageSwap {
            first,
            second,
            transaction,
        }))
    }

    /// Replaces a remote image path after it has been materialized locally.
    ///
    /// The original path predicate prevents a slow downloader from overwriting
    /// a newer metadata refresh. A missing return row means the image changed
    /// concurrently and callers should reload it.
    ///
    /// # Errors
    ///
    /// Returns database or corrupt-row errors.
    pub async fn relocate_if_path_matches(
        &self,
        image: &BaseItemImage,
        path: &str,
        date_modified: DateTime<Utc>,
    ) -> Result<Option<BaseItemImage>, BaseItemImageStoreError> {
        if path.trim().is_empty() {
            return Err(BaseItemImageStoreError::EmptyPath);
        }
        let image_index = i32::try_from(image.image_index).map_err(|_| {
            BaseItemImageStoreError::ImageIndexOutOfRange {
                value: image.image_index,
            }
        })?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.base_item_images
            SET path = $5, date_modified = $6
            WHERE item_id = $1
              AND image_type = $2
              AND image_index = $3
              AND path = $4
            RETURNING item_id, image_type, image_index, path, date_modified,
                      width, height, blurhash
            ",
            [
                image.item_id.into(),
                image.image_type.as_i16().into(),
                image_index.into(),
                image.path.as_str().into(),
                path.to_owned().into(),
                date_modified.into(),
            ],
        );
        base_item_image::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .map(BaseItemImage::try_from)
            .transpose()
    }

    /// Conditionally refreshes local image dimensions and `BlurHash` metadata.
    ///
    /// The path and previous modification timestamp form an optimistic guard,
    /// preventing a slow decoder from overwriting a concurrently replaced image.
    ///
    /// # Errors
    ///
    /// Returns validation, database, or corrupt-row errors.
    pub async fn refresh_local_metadata_if_matches(
        &self,
        image: &BaseItemImage,
        date_modified: DateTime<Utc>,
        width: u32,
        height: u32,
        blurhash: &str,
    ) -> Result<Option<BaseItemImage>, BaseItemImageStoreError> {
        let image_index = i32::try_from(image.image_index).map_err(|_| {
            BaseItemImageStoreError::ImageIndexOutOfRange {
                value: image.image_index,
            }
        })?;
        let width = validate_dimension("width", Some(width))?.ok_or_else(|| {
            BaseItemImageStoreError::InvalidDimension {
                field: "width",
                value: width,
            }
        })?;
        let height = validate_dimension("height", Some(height))?.ok_or_else(|| {
            BaseItemImageStoreError::InvalidDimension {
                field: "height",
                value: height,
            }
        })?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.base_item_images
            SET date_modified = $6, width = $7, height = $8, blurhash = $9
            WHERE item_id = $1
              AND image_type = $2
              AND image_index = $3
              AND path = $4
              AND date_modified = $5
            RETURNING item_id, image_type, image_index, path, date_modified,
                      width, height, blurhash
            ",
            [
                image.item_id.into(),
                image.image_type.as_i16().into(),
                image_index.into(),
                image.path.as_str().into(),
                image.date_modified.into(),
                date_modified.into(),
                width.into(),
                height.into(),
                blurhash.to_owned().into(),
            ],
        );
        base_item_image::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .map(BaseItemImage::try_from)
            .transpose()
    }

    /// Replaces index zero for a single-image type or appends a backdrop.
    ///
    /// The owning item row is locked before a backdrop index is selected, so
    /// concurrent uploads cannot choose the same `MAX(image_index) + 1` key.
    /// `PostgreSQL`'s composite primary key then handles the single-image upsert.
    ///
    /// # Errors
    ///
    /// Returns typed input, missing-item, unsupported-type, corrupt-row, or
    /// database errors.
    pub async fn set_or_append(
        &self,
        item_id: Uuid,
        mut image: NewBaseItemImage,
    ) -> Result<StoredImageMutation, BaseItemImageStoreError> {
        if image.image_type == BaseItemImageType::Chapter {
            return Err(BaseItemImageStoreError::UnsupportedUploadImageType {
                image_type: image.image_type,
            });
        }
        image.image_index = 0;
        let image_type = image.image_type;
        let validated = validate_image(
            image.image_type,
            image.image_index,
            Cow::Owned(image.path),
            image.date_modified,
            image.width,
            image.height,
            image.blurhash.map(Cow::Owned),
        )?;
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(BaseItemImageStoreError::BaseItemNotFound { item_id });
        }

        let image_index = if image_type == BaseItemImageType::Backdrop {
            let row = transaction
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r"
                    SELECT COALESCE(MAX(image_index)::bigint + 1, 0) AS next_index
                    FROM jellyfin.base_item_images
                    WHERE item_id = $1 AND image_type = $2
                    ",
                    [item_id.into(), image_type.as_i16().into()],
                ))
                .await?
                .ok_or_else(|| DbErr::Custom("backdrop index aggregate was missing".to_owned()))?;
            let next_index: i64 = row.try_get("", "next_index")?;
            i32::try_from(next_index).map_err(|_| {
                BaseItemImageStoreError::ImageIndexOutOfRange {
                    value: u32::try_from(next_index).unwrap_or(u32::MAX),
                }
            })?
        } else {
            0
        };

        let replaced =
            base_item_image::Entity::find_by_id((item_id, image_type.as_i16(), image_index))
                .one(&transaction)
                .await?
                .map(BaseItemImage::try_from)
                .transpose()?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.base_item_images (
                item_id, image_type, image_index, path, date_modified,
                width, height, blurhash
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (item_id, image_type, image_index) DO UPDATE
                SET path = EXCLUDED.path,
                    date_modified = EXCLUDED.date_modified,
                    width = EXCLUDED.width,
                    height = EXCLUDED.height,
                    blurhash = EXCLUDED.blurhash
            RETURNING item_id, image_type, image_index, path, date_modified,
                      width, height, blurhash
            ",
            [
                item_id.into(),
                validated.image_type.into(),
                image_index.into(),
                validated.path.into_owned().into(),
                validated.date_modified.into(),
                validated.width.into(),
                validated.height.into(),
                validated.blurhash.map(Cow::into_owned).into(),
            ],
        );
        let current = base_item_image::Model::find_by_statement(statement)
            .one(&transaction)
            .await?
            .ok_or_else(|| DbErr::Custom("image upsert did not return its row".to_owned()))?
            .try_into()?;
        transaction.commit().await?;
        Ok(StoredImageMutation { current, replaced })
    }

    /// Atomically replaces every image for one base item.
    ///
    /// A row lock on the owning item serializes competing replacements. The
    /// input is expanded with `jsonb_to_recordset`, stale rows are deleted, and
    /// the requested rows are bulk-upserted with `RETURNING` in one transaction.
    ///
    /// # Errors
    ///
    /// Returns typed validation, missing-item, corrupt-row, or database errors.
    pub async fn replace(
        &self,
        item_id: Uuid,
        images: &[NewBaseItemImage],
    ) -> Result<Vec<BaseItemImage>, BaseItemImageStoreError> {
        let validated = validate_images(images)?;
        let payload = Value::Array(validated.iter().map(ValidatedImage::to_json).collect());
        let transaction = self.database.begin().await?;

        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(BaseItemImageStoreError::BaseItemNotFound { item_id });
        }

        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH input AS (
                    SELECT image_type, image_index
                    FROM jsonb_to_recordset($2::jsonb) AS image(
                        image_type smallint,
                        image_index integer
                    )
                )
                DELETE FROM jellyfin.base_item_images AS stored
                WHERE stored.item_id = $1
                  AND NOT EXISTS (
                      SELECT 1
                      FROM input
                      WHERE input.image_type = stored.image_type
                        AND input.image_index = stored.image_index
                  )
                ",
                [item_id.into(), payload.clone().into()],
            ))
            .await?;

        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            WITH input AS (
                SELECT image_type, image_index, path, date_modified,
                       width, height, blurhash
                FROM jsonb_to_recordset($2::jsonb) AS image(
                    image_type smallint,
                    image_index integer,
                    path text,
                    date_modified timestamptz,
                    width integer,
                    height integer,
                    blurhash text
                )
            ), upserted AS (
                INSERT INTO jellyfin.base_item_images (
                    item_id, image_type, image_index, path, date_modified,
                    width, height, blurhash
                )
                SELECT $1, image_type, image_index, path, date_modified,
                       width, height, blurhash
                FROM input
                ON CONFLICT (item_id, image_type, image_index) DO UPDATE
                    SET path = EXCLUDED.path,
                        date_modified = EXCLUDED.date_modified,
                        width = EXCLUDED.width,
                        height = EXCLUDED.height,
                        blurhash = EXCLUDED.blurhash
                RETURNING item_id, image_type, image_index, path, date_modified,
                          width, height, blurhash
            )
            SELECT item_id, image_type, image_index, path, date_modified,
                   width, height, blurhash
            FROM upserted
            ORDER BY image_type, image_index
            ",
            [item_id.into(), payload.into()],
        );
        let rows = base_item_image::Model::find_by_statement(statement)
            .all(&transaction)
            .await?;
        let images = rows
            .into_iter()
            .map(BaseItemImage::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await?;
        Ok(images)
    }
}

#[derive(Debug)]
struct ValidatedImage<'a> {
    image_type: i16,
    image_index: i32,
    path: Cow<'a, str>,
    date_modified: DateTime<Utc>,
    width: Option<i32>,
    height: Option<i32>,
    blurhash: Option<Cow<'a, str>>,
}

impl ValidatedImage<'_> {
    fn to_json(&self) -> Value {
        json!({
            "image_type": self.image_type,
            "image_index": self.image_index,
            "path": self.path.as_ref(),
            "date_modified": self.date_modified.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            "width": self.width,
            "height": self.height,
            "blurhash": self.blurhash.as_deref(),
        })
    }
}

fn validate_images(
    images: &[NewBaseItemImage],
) -> Result<Vec<ValidatedImage<'_>>, BaseItemImageStoreError> {
    let mut keys = HashSet::with_capacity(images.len());
    images
        .iter()
        .map(|image| {
            if !keys.insert((image.image_type, image.image_index)) {
                return Err(BaseItemImageStoreError::DuplicateImage {
                    image_type: image.image_type,
                    image_index: image.image_index,
                });
            }
            validate_image(
                image.image_type,
                image.image_index,
                Cow::Borrowed(&image.path),
                image.date_modified,
                image.width,
                image.height,
                image.blurhash.as_deref().map(Cow::Borrowed),
            )
        })
        .collect()
}

fn validate_image<'a>(
    image_type: BaseItemImageType,
    image_index: u32,
    path: Cow<'a, str>,
    date_modified: DateTime<Utc>,
    width: Option<u32>,
    height: Option<u32>,
    blurhash: Option<Cow<'a, str>>,
) -> Result<ValidatedImage<'a>, BaseItemImageStoreError> {
    if path.trim().is_empty() {
        return Err(BaseItemImageStoreError::EmptyPath);
    }
    let image_index = i32::try_from(image_index)
        .map_err(|_| BaseItemImageStoreError::ImageIndexOutOfRange { value: image_index })?;
    Ok(ValidatedImage {
        image_type: image_type.as_i16(),
        image_index,
        path,
        date_modified,
        width: validate_dimension("width", width)?,
        height: validate_dimension("height", height)?,
        blurhash,
    })
}

fn validate_dimension(
    field: &'static str,
    value: Option<u32>,
) -> Result<Option<i32>, BaseItemImageStoreError> {
    value
        .map(|value| {
            if value == 0 {
                return Err(BaseItemImageStoreError::InvalidDimension { field, value });
            }
            i32::try_from(value)
                .map_err(|_| BaseItemImageStoreError::InvalidDimension { field, value })
        })
        .transpose()
}

impl TryFrom<base_item_image::Model> for BaseItemImage {
    type Error = BaseItemImageStoreError;

    fn try_from(row: base_item_image::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            item_id: row.item_id,
            image_type: row.image_type.try_into()?,
            image_index: u32::try_from(row.image_index).map_err(|_| {
                BaseItemImageStoreError::CorruptRow {
                    field: "image_index",
                    value: row.image_index,
                }
            })?,
            path: row.path,
            date_modified: row.date_modified,
            width: stored_dimension("width", row.width)?,
            height: stored_dimension("height", row.height)?,
            blurhash: row.blurhash,
        })
    }
}

fn stored_dimension(
    field: &'static str,
    value: Option<i32>,
) -> Result<Option<u32>, BaseItemImageStoreError> {
    value
        .map(|value| {
            u32::try_from(value).map_err(|_| BaseItemImageStoreError::CorruptRow { field, value })
        })
        .transpose()
}
