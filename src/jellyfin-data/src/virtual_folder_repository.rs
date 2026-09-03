use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseTransaction,
    DbBackend, DbErr, EntityTrait, LoaderTrait, QueryFilter, QueryOrder, SqlErr, Statement,
    TransactionTrait,
};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{media_path, virtual_folder};

const PATH_MUTATION_LOCK_KEY: i64 = 0x5646_5041_5448_5054;

#[derive(Debug, Clone, PartialEq)]
pub struct NewVirtualFolder {
    pub name: String,
    pub collection_type: Option<String>,
    pub library_options: Value,
    pub refresh_requested: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMediaPath {
    pub path: String,
    pub normalized_path: String,
    pub ancestors: Vec<String>,
    pub path_info: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VirtualFolderWithPaths {
    pub folder: virtual_folder::Model,
    pub paths: Vec<media_path::Model>,
}

#[derive(Debug, Error)]
pub enum VirtualFolderError {
    #[error("virtual folder name cannot be empty")]
    InvalidName,
    #[error("virtual folder name already exists")]
    DuplicateName,
    #[error("virtual folder was not found")]
    NotFound,
    #[error("media path already exists or overlaps another path")]
    PathOverlap,
    #[error("media path was not found")]
    PathNotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct VirtualFolderRepository {
    database: crate::SharedDatabase,
}

impl VirtualFolderRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Creates a virtual folder and its media paths atomically.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, or database errors.
    pub async fn create(
        &self,
        folder: NewVirtualFolder,
        paths: Vec<NewMediaPath>,
    ) -> Result<VirtualFolderWithPaths, VirtualFolderError> {
        let normalized_name = normalized_name(&folder.name)?;
        let transaction = self.database.begin().await?;
        let inserted = virtual_folder::Entity::insert(virtual_folder::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set(folder.name),
            normalized_name: Set(normalized_name),
            collection_type: Set(folder.collection_type),
            library_options: Set(folder.library_options),
            refresh_requested: Set(folder.refresh_requested),
            ..Default::default()
        })
        .exec_with_returning(&transaction)
        .await
        .map_err(map_database_error)?;
        let paths = insert_paths(&transaction, inserted.id, paths).await?;
        transaction.commit().await?;
        Ok(VirtualFolderWithPaths {
            folder: inserted,
            paths,
        })
    }

    /// Lists virtual folders with stable name and path ordering.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn list(&self) -> Result<Vec<VirtualFolderWithPaths>, VirtualFolderError> {
        let folders = virtual_folder::Entity::find()
            .order_by_asc(virtual_folder::Column::NormalizedName)
            .order_by_asc(virtual_folder::Column::Id)
            .all(self.database.as_ref())
            .await?;
        let related_paths = folders
            .load_many(media_path::Entity, self.database.as_ref())
            .await?;
        Ok(folders
            .into_iter()
            .zip(related_paths)
            .map(|(folder, mut paths)| {
                paths.sort_by(|left, right| left.normalized_path.cmp(&right.normalized_path));
                VirtualFolderWithPaths { folder, paths }
            })
            .collect())
    }

    /// Finds a folder by Jellyfin's Unicode-aware normalized name.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn get_by_name(
        &self,
        name: &str,
    ) -> Result<Option<VirtualFolderWithPaths>, VirtualFolderError> {
        let normalized = normalized_name(name)?;
        let Some(folder) = virtual_folder::Entity::find()
            .filter(virtual_folder::Column::NormalizedName.eq(normalized))
            .one(self.database.as_ref())
            .await?
        else {
            return Ok(None);
        };
        let paths = media_path::Entity::find()
            .filter(media_path::Column::VirtualFolderId.eq(folder.id))
            .order_by_asc(media_path::Column::NormalizedPath)
            .all(self.database.as_ref())
            .await?;
        Ok(Some(VirtualFolderWithPaths { folder, paths }))
    }

    /// Renames a folder while preserving its stable item identifier.
    ///
    /// # Errors
    ///
    /// Returns `NotFound`, `DuplicateName`, validation, or database errors.
    pub async fn rename(
        &self,
        name: &str,
        new_name: &str,
        refresh_requested: bool,
    ) -> Result<virtual_folder::Model, VirtualFolderError> {
        let current = normalized_name(name)?;
        let replacement = normalized_name(new_name)?;
        let Some(model) = virtual_folder::Entity::find()
            .filter(virtual_folder::Column::NormalizedName.eq(current))
            .one(self.database.as_ref())
            .await?
        else {
            return Err(VirtualFolderError::NotFound);
        };
        let mut active: virtual_folder::ActiveModel = model.into();
        active.name = Set(new_name.to_owned());
        active.normalized_name = Set(replacement);
        active.refresh_requested = Set(refresh_requested);
        active
            .update(self.database.as_ref())
            .await
            .map_err(map_database_error)
    }

    /// Deletes a folder and cascades all media paths.
    ///
    /// # Errors
    ///
    /// Returns `NotFound`, validation, or database errors.
    pub async fn delete(
        &self,
        name: &str,
        _refresh_requested: bool,
    ) -> Result<(), VirtualFolderError> {
        let normalized = normalized_name(name)?;
        let Some(folder) = virtual_folder::Entity::find()
            .filter(virtual_folder::Column::NormalizedName.eq(normalized))
            .one(self.database.as_ref())
            .await?
        else {
            return Err(VirtualFolderError::NotFound);
        };
        let transaction = self.database.begin().await?;
        // Delete all media items belonging to this library.
        // The collection folder shares the virtual folder's id as the base item id.
        // ON DELETE CASCADE on parent_id handles all descendant items.
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM jellyfin.base_items WHERE id = $1::uuid",
                [folder.id.into()],
            ))
            .await?;
        // Delete the virtual folder (cascades to media_paths).
        virtual_folder::Entity::delete_by_id(folder.id)
            .exec(&transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Replaces the JSONB library options of a folder.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` or a database error.
    pub async fn update_options(&self, id: Uuid, options: Value) -> Result<(), VirtualFolderError> {
        let Some(model) = virtual_folder::Entity::find_by_id(id)
            .one(self.database.as_ref())
            .await?
        else {
            return Err(VirtualFolderError::NotFound);
        };
        let mut active: virtual_folder::ActiveModel = model.into();
        active.library_options = Set(options);
        active.update(self.database.as_ref()).await?;
        Ok(())
    }

    /// Adds a non-overlapping canonical media path.
    ///
    /// # Errors
    ///
    /// Returns folder, path conflict, or database errors.
    pub async fn add_path(
        &self,
        name: &str,
        path: NewMediaPath,
        refresh_requested: bool,
    ) -> Result<media_path::Model, VirtualFolderError> {
        let normalized = normalized_name(name)?;
        let transaction = self.database.begin().await?;
        let Some(folder) = virtual_folder::Entity::find()
            .filter(virtual_folder::Column::NormalizedName.eq(normalized))
            .one(&transaction)
            .await?
        else {
            return Err(VirtualFolderError::NotFound);
        };
        let inserted = insert_paths(&transaction, folder.id, vec![path])
            .await?
            .pop()
            .ok_or_else(|| {
                VirtualFolderError::Database(DbErr::Custom(
                    "media path insertion returned no row".to_owned(),
                ))
            })?;
        set_refresh_requested(&transaction, folder, refresh_requested).await?;
        transaction.commit().await?;
        Ok(inserted)
    }

    /// Updates metadata for an existing canonical media path.
    ///
    /// # Errors
    ///
    /// Returns folder/path not found or database errors.
    pub async fn update_path(
        &self,
        name: &str,
        normalized_path: &str,
        path_info: Value,
    ) -> Result<(), VirtualFolderError> {
        let folder = self
            .get_by_name(name)
            .await?
            .ok_or(VirtualFolderError::NotFound)?;
        let Some(path) = folder
            .paths
            .into_iter()
            .find(|path| path.normalized_path == normalized_path)
        else {
            return Err(VirtualFolderError::PathNotFound);
        };
        let mut active: media_path::ActiveModel = path.into();
        active.path_info = Set(path_info);
        active.update(self.database.as_ref()).await?;
        Ok(())
    }

    /// Removes one exact canonical path from a folder.
    ///
    /// # Errors
    ///
    /// Returns folder/path not found or database errors.
    pub async fn remove_path(
        &self,
        name: &str,
        normalized_path: &str,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderError> {
        let normalized = normalized_name(name)?;
        let transaction = self.database.begin().await?;
        let Some(folder) = virtual_folder::Entity::find()
            .filter(virtual_folder::Column::NormalizedName.eq(normalized))
            .one(&transaction)
            .await?
        else {
            return Err(VirtualFolderError::NotFound);
        };
        let result = media_path::Entity::delete_many()
            .filter(media_path::Column::VirtualFolderId.eq(folder.id))
            .filter(media_path::Column::NormalizedPath.eq(normalized_path))
            .exec(&transaction)
            .await?;
        if result.rows_affected == 0 {
            return Err(VirtualFolderError::PathNotFound);
        }
        set_refresh_requested(&transaction, folder, refresh_requested).await?;
        transaction.commit().await?;
        Ok(())
    }
}

async fn insert_paths(
    transaction: &DatabaseTransaction,
    folder_id: Uuid,
    paths: Vec<NewMediaPath>,
) -> Result<Vec<media_path::Model>, VirtualFolderError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    acquire_path_lock(transaction).await?;
    let mut inserted = Vec::with_capacity(paths.len());
    for path in paths {
        if path_overlaps(transaction, &path).await? {
            return Err(VirtualFolderError::PathOverlap);
        }
        let model = media_path::Entity::insert(media_path::ActiveModel {
            id: Set(Uuid::new_v4()),
            virtual_folder_id: Set(folder_id),
            path: Set(path.path),
            normalized_path: Set(path.normalized_path),
            path_ancestors: Set(json!(path.ancestors)),
            path_info: Set(path.path_info),
            ..Default::default()
        })
        .exec_with_returning(transaction)
        .await
        .map_err(map_database_error)?;
        inserted.push(model);
    }
    Ok(inserted)
}

async fn acquire_path_lock(transaction: &DatabaseTransaction) -> Result<(), DbErr> {
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [PATH_MUTATION_LOCK_KEY.into()],
        ))
        .await?;
    Ok(())
}

async fn path_overlaps(
    transaction: &DatabaseTransaction,
    candidate: &NewMediaPath,
) -> Result<bool, DbErr> {
    Ok(transaction
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"SELECT 1
               FROM jellyfin.media_paths
              WHERE normalized_path IN (
                        SELECT jsonb_array_elements_text($1::jsonb)
                    )
                 OR path_ancestors ? $2
              LIMIT 1",
            [
                json!(candidate.ancestors).into(),
                candidate.normalized_path.as_str().into(),
            ],
        ))
        .await?
        .is_some())
}

async fn set_refresh_requested(
    transaction: &DatabaseTransaction,
    folder: virtual_folder::Model,
    refresh_requested: bool,
) -> Result<(), DbErr> {
    let mut active: virtual_folder::ActiveModel = folder.into();
    active.refresh_requested = Set(refresh_requested);
    active.update(transaction).await?;
    Ok(())
}

fn normalized_name(name: &str) -> Result<String, VirtualFolderError> {
    if name.is_empty() || name.trim() != name {
        return Err(VirtualFolderError::InvalidName);
    }
    let normalized = name.clean_value();
    if normalized.is_empty() {
        Err(VirtualFolderError::InvalidName)
    } else {
        Ok(normalized)
    }
}

fn map_database_error(error: DbErr) -> VirtualFolderError {
    let message = error.to_string();
    if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        if message.contains("virtual_folders_normalized_name_key") {
            return VirtualFolderError::DuplicateName;
        }
        if message.contains("media_paths_normalized_path_key") {
            return VirtualFolderError::PathOverlap;
        }
    }
    VirtualFolderError::Database(error)
}
