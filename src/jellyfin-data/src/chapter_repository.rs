use std::collections::HashMap;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, IntoActiveModel,
    QueryFilter, QueryOrder, Set, Statement, TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use chrono::{DateTime, Utc};

use crate::entities::chapter;

const CHAPTER_WRITE_BATCH_SIZE: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewChapter {
    pub index_number: i32,
    pub start_position_ticks: i64,
    pub end_position_ticks: i64,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterRecord {
    pub id: Uuid,
    pub item_id: Uuid,
    pub index_number: i32,
    pub start_position_ticks: i64,
    pub end_position_ticks: i64,
    pub name: Option<String>,
    pub image_path: Option<String>,
    pub image_date_modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Error)]
pub enum ChapterStoreError {
    #[error("chapter {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("chapter {field} exceeds its {max} character limit")]
    FieldTooLong { field: &'static str, max: usize },
    #[error("chapter end must not be before its start")]
    InvalidRange,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct ChapterRepository {
    database: crate::SharedDatabase,
}

impl ChapterRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Atomically replaces all chapters for one item in bounded write batches.
    ///
    /// The owning item serializes concurrent replacements, including an empty replacement.
    /// Returned records retain input order; chapter reads use start-position order.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn replace(
        &self,
        item_id: Uuid,
        chapters: Vec<NewChapter>,
    ) -> Result<Vec<ChapterRecord>, ChapterStoreError> {
        for chapter in &chapters {
            validate_chapter(chapter)?;
        }
        let transaction = self.database.begin().await?;
        // Official ChapterRepository.SaveChapters deletes and inserts inside one transaction.
        // PostgreSQL's statement snapshots also need an owner lock: competing replacements of
        // an initially empty chapter set must not both insert their own chapters after deleting.
        transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR NO KEY UPDATE",
                [item_id.into()],
            ))
            .await?;
        chapter::Entity::delete_many()
            .filter(chapter::Column::ItemId.eq(item_id))
            .exec(&transaction)
            .await?;
        let mut records = Vec::with_capacity(chapters.len());
        let mut chapters = chapters.into_iter();
        loop {
            let batch = chapters
                .by_ref()
                .take(CHAPTER_WRITE_BATCH_SIZE)
                .map(|chapter| chapter::Model {
                    id: Uuid::new_v4(),
                    item_id,
                    index_number: chapter.index_number,
                    start_position_ticks: chapter.start_position_ticks,
                    end_position_ticks: chapter.end_position_ticks,
                    name: chapter.name,
                    image_path: None,
                    image_date_modified: None,
                })
                .collect::<Vec<_>>();
            if batch.is_empty() {
                break;
            }
            // All persisted fields are supplied above, so RETURNING would only buffer and
            // transfer a second copy of each chapter. Bound SQL parameters and temporary models.
            chapter::Entity::insert_many(
                batch
                    .iter()
                    .cloned()
                    .map(IntoActiveModel::into_active_model),
            )
            .exec_without_returning(&transaction)
            .await?;
            records.extend(batch.into_iter().map(ChapterRecord::from));
        }
        transaction.commit().await?;
        Ok(records)
    }

    /// Lists chapters for one item in start-position order.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn list_for_item(
        &self,
        item_id: Uuid,
    ) -> Result<Vec<ChapterRecord>, ChapterStoreError> {
        Ok(chapter::Entity::find()
            .filter(chapter::Column::ItemId.eq(item_id))
            .order_by_asc(chapter::Column::StartPositionTicks)
            .order_by_asc(chapter::Column::IndexNumber)
            .order_by_asc(chapter::Column::Id)
            .all(self.database.as_ref())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Lists chapters for several items in one query, grouped by item id.
    ///
    /// Every requested item has an entry, including items without chapters. Chapters within each
    /// entry use the official DTO order: start position first, with persisted index and id as
    /// deterministic tie-breakers.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn list_many(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<ChapterRecord>>, ChapterStoreError> {
        let mut grouped = item_ids
            .iter()
            .copied()
            .map(|item_id| (item_id, Vec::new()))
            .collect::<HashMap<_, _>>();
        if grouped.is_empty() {
            return Ok(grouped);
        }

        let chapters = chapter::Entity::find()
            .filter(chapter::Column::ItemId.is_in(grouped.keys().copied()))
            .order_by_asc(chapter::Column::ItemId)
            .order_by_asc(chapter::Column::StartPositionTicks)
            .order_by_asc(chapter::Column::IndexNumber)
            .order_by_asc(chapter::Column::Id)
            .all(self.database.as_ref())
            .await?;
        for chapter in chapters {
            grouped
                .entry(chapter.item_id)
                .or_default()
                .push(chapter.into());
        }
        Ok(grouped)
    }

    /// Gets one chapter by its persisted index within an item.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn get(
        &self,
        item_id: Uuid,
        index_number: i32,
    ) -> Result<Option<ChapterRecord>, ChapterStoreError> {
        Ok(chapter::Entity::find()
            .filter(chapter::Column::ItemId.eq(item_id))
            .filter(chapter::Column::IndexNumber.eq(index_number))
            .one(self.database.as_ref())
            .await?
            .map(Into::into))
    }

    /// Updates the generated-image metadata for one chapter.
    ///
    /// # Errors
    ///
    /// Returns a database error when the update fails.
    pub async fn set_image_data(
        &self,
        chapter_id: Uuid,
        image_path: impl Into<String>,
        image_date_modified: DateTime<Utc>,
    ) -> Result<(), ChapterStoreError> {
        chapter::ActiveModel {
            id: Set(chapter_id),
            image_path: Set(Some(image_path.into())),
            image_date_modified: Set(Some(image_date_modified)),
            ..Default::default()
        }
        .update(self.database.as_ref())
        .await?;
        Ok(())
    }
}

impl From<chapter::Model> for ChapterRecord {
    fn from(chapter: chapter::Model) -> Self {
        Self {
            id: chapter.id,
            item_id: chapter.item_id,
            index_number: chapter.index_number,
            start_position_ticks: chapter.start_position_ticks,
            end_position_ticks: chapter.end_position_ticks,
            name: chapter.name,
            image_path: chapter.image_path,
            image_date_modified: chapter.image_date_modified,
        }
    }
}

fn validate_chapter(chapter: &NewChapter) -> Result<(), ChapterStoreError> {
    if let Some(name) = chapter.name.as_deref() {
        if name.trim().is_empty() {
            return Err(ChapterStoreError::EmptyField("name"));
        }
        if name.chars().count() > 1024 {
            return Err(ChapterStoreError::FieldTooLong {
                field: "name",
                max: 1024,
            });
        }
    }
    if chapter.end_position_ticks < chapter.start_position_ticks {
        return Err(ChapterStoreError::InvalidRange);
    }
    Ok(())
}
