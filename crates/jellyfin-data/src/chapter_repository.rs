use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use chrono::{DateTime, Utc};

use crate::entities::chapter;

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
    database: DatabaseConnection,
}

impl ChapterRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Replaces all chapters for one item in index order.
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
        chapter::Entity::delete_many()
            .filter(chapter::Column::ItemId.eq(item_id))
            .exec(&transaction)
            .await?;
        let mut records = Vec::with_capacity(chapters.len());
        for chapter in chapters {
            let id = Uuid::new_v4();
            let model = chapter::ActiveModel {
                id: Set(id),
                item_id: Set(item_id),
                index_number: Set(chapter.index_number),
                start_position_ticks: Set(chapter.start_position_ticks),
                end_position_ticks: Set(chapter.end_position_ticks),
                name: Set(chapter.name),
                image_path: Set(None),
                image_date_modified: Set(None),
            }
            .insert(&transaction)
            .await?;
            records.push(ChapterRecord {
                id: model.id,
                item_id: model.item_id,
                index_number: model.index_number,
                start_position_ticks: model.start_position_ticks,
                end_position_ticks: model.end_position_ticks,
                name: model.name,
                image_path: model.image_path,
                image_date_modified: model.image_date_modified,
            });
        }
        transaction.commit().await?;
        Ok(records)
    }

    /// Lists chapters for one item in index order.
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
            .order_by_asc(chapter::Column::IndexNumber)
            .all(&self.database)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
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
        .update(&self.database)
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
