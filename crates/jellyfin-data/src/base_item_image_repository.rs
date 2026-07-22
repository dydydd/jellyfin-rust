use std::collections::HashSet;

use chrono::{DateTime, SecondsFormat, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, Statement, TransactionTrait,
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
    database: DatabaseConnection,
}

impl BaseItemImageRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
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
            .all(&self.database)
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
            .all(&self.database)
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
            .one(&self.database)
            .await?
            .map(BaseItemImage::try_from)
            .transpose()
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
struct ValidatedImage {
    image_type: i16,
    image_index: i32,
    path: String,
    date_modified: DateTime<Utc>,
    width: Option<i32>,
    height: Option<i32>,
    blurhash: Option<String>,
}

impl ValidatedImage {
    fn to_json(&self) -> Value {
        json!({
            "image_type": self.image_type,
            "image_index": self.image_index,
            "path": self.path,
            "date_modified": self.date_modified.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            "width": self.width,
            "height": self.height,
            "blurhash": self.blurhash,
        })
    }
}

fn validate_images(
    images: &[NewBaseItemImage],
) -> Result<Vec<ValidatedImage>, BaseItemImageStoreError> {
    let mut keys = HashSet::with_capacity(images.len());
    images
        .iter()
        .map(|image| {
            if image.path.trim().is_empty() {
                return Err(BaseItemImageStoreError::EmptyPath);
            }
            if !keys.insert((image.image_type, image.image_index)) {
                return Err(BaseItemImageStoreError::DuplicateImage {
                    image_type: image.image_type,
                    image_index: image.image_index,
                });
            }
            let image_index = i32::try_from(image.image_index).map_err(|_| {
                BaseItemImageStoreError::ImageIndexOutOfRange {
                    value: image.image_index,
                }
            })?;
            let width = validate_dimension("width", image.width)?;
            let height = validate_dimension("height", image.height)?;
            Ok(ValidatedImage {
                image_type: image.image_type.as_i16(),
                image_index,
                path: image.path.clone(),
                date_modified: image.date_modified,
                width,
                height,
                blurhash: image.blurhash.clone(),
            })
        })
        .collect()
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
