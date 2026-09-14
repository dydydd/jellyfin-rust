use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, DbErr, EntityTrait, QueryFilter, QueryOrder, Set,
    sea_query::Expr,
};
use thiserror::Error;

use crate::entities::emby_camera_upload;

const DEVICE_ID_MAX: usize = 256;
const UPLOAD_ID_MAX: usize = 1024;
const NAME_MAX: usize = 512;
const ALBUM_MAX: usize = 512;
const MIME_TYPE_MAX: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEmbyCameraUpload {
    pub device_id: String,
    pub upload_id: String,
    pub name: String,
    pub album: String,
    pub mime_type: Option<String>,
    pub date_created: Option<DateTime<Utc>>,
    pub stored_path: String,
}

#[derive(Debug, Error)]
pub enum EmbyCameraUploadStoreError {
    #[error("camera upload {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("camera upload {field} exceeds its {max} character limit")]
    FieldTooLong { field: &'static str, max: usize },
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct EmbyCameraUploadRepository {
    database: crate::SharedDatabase,
}

impl EmbyCameraUploadRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Returns history for the current reported device, preserving the
    /// official append order and case-insensitive device-id comparison.
    ///
    /// # Errors
    ///
    /// Returns validation failures for an invalid device id or a database
    /// error when the history cannot be loaded.
    pub async fn list(
        &self,
        device_id: &str,
    ) -> Result<Vec<emby_camera_upload::Model>, EmbyCameraUploadStoreError> {
        validate_required("device id", device_id, DEVICE_ID_MAX)?;
        Ok(emby_camera_upload::Entity::find()
            .filter(Expr::cust_with_values(
                "lower(device_id) = lower($1::text)",
                [device_id.to_owned()],
            ))
            .order_by_asc(emby_camera_upload::Column::Id)
            .all(self.database.as_ref())
            .await?)
    }

    /// Appends a completed upload. Callers publish the file atomically before
    /// this insert and remove that unique file if persistence fails.
    ///
    /// # Errors
    ///
    /// Returns validation failures for invalid metadata or a database error
    /// when the append cannot be persisted.
    pub async fn insert(
        &self,
        upload: NewEmbyCameraUpload,
    ) -> Result<emby_camera_upload::Model, EmbyCameraUploadStoreError> {
        validate_required("device id", &upload.device_id, DEVICE_ID_MAX)?;
        validate_required("id", &upload.upload_id, UPLOAD_ID_MAX)?;
        validate_required("name", &upload.name, NAME_MAX)?;
        validate_optional("album", &upload.album, ALBUM_MAX)?;
        if let Some(mime_type) = upload.mime_type.as_deref() {
            validate_optional("MIME type", mime_type, MIME_TYPE_MAX)?;
        }
        validate_required("stored path", &upload.stored_path, usize::MAX)?;
        Ok(emby_camera_upload::ActiveModel {
            id: NotSet,
            device_id: Set(upload.device_id),
            upload_id: Set(upload.upload_id),
            name: Set(upload.name),
            album: Set(upload.album),
            mime_type: Set(upload.mime_type),
            date_created: Set(upload.date_created),
            stored_path: Set(upload.stored_path),
            created_at: Set(Utc::now()),
        }
        .insert(self.database.as_ref())
        .await?)
    }
}

fn validate_required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), EmbyCameraUploadStoreError> {
    if value.trim().is_empty() {
        return Err(EmbyCameraUploadStoreError::EmptyField(field));
    }
    validate_optional(field, value, max)
}

fn validate_optional(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), EmbyCameraUploadStoreError> {
    if value.chars().count() > max {
        return Err(EmbyCameraUploadStoreError::FieldTooLong { field, max });
    }
    Ok(())
}
