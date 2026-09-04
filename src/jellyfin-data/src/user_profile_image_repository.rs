use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter, Statement,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::user_profile_image;

const MAX_PATH_LENGTH: usize = 512;

/// Detached user profile-image input without a database-generated image key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewUserProfileImage {
    pub user_id: Uuid,
    pub path: String,
    pub last_modified: DateTime<Utc>,
}

/// User profile-image persistence failure.
#[derive(Debug, Error)]
pub enum UserProfileImageStoreError {
    #[error("user profile image path cannot be empty")]
    EmptyPath,
    #[error("user profile image path exceeds its {max} character limit")]
    PathTooLong { max: usize },
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed one-to-one user profile-image storage.
#[derive(Clone)]
pub struct UserProfileImageRepository {
    database: crate::SharedDatabase,
}

impl UserProfileImageRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Atomically inserts or replaces the image path for one user.
    ///
    /// # Errors
    ///
    /// Returns an empty-path validation error or a database error, including
    /// a foreign-key failure when the user does not exist.
    pub async fn upsert(
        &self,
        image: NewUserProfileImage,
    ) -> Result<user_profile_image::Model, UserProfileImageStoreError> {
        if image.path.trim().is_empty() {
            return Err(UserProfileImageStoreError::EmptyPath);
        }
        if image.path.chars().count() > MAX_PATH_LENGTH {
            return Err(UserProfileImageStoreError::PathTooLong {
                max: MAX_PATH_LENGTH,
            });
        }
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.user_profile_images (user_id, path, last_modified)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id) DO UPDATE
                SET path = EXCLUDED.path,
                    last_modified = EXCLUDED.last_modified
            RETURNING user_id, path, last_modified
            ",
            [
                image.user_id.into(),
                image.path.into(),
                image.last_modified.into(),
            ],
        );
        user_profile_image::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| {
                UserProfileImageStoreError::Database(DbErr::RecordNotFound(
                    "user profile image upsert returned no row".to_owned(),
                ))
            })
    }

    /// Loads the profile image for one user.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn get(
        &self,
        user_id: Uuid,
    ) -> Result<Option<user_profile_image::Model>, UserProfileImageStoreError> {
        Ok(user_profile_image::Entity::find_by_id(user_id)
            .one(self.database.as_ref())
            .await?)
    }

    /// Loads profile images for a set of users in one query.
    ///
    /// Duplicate user identifiers do not duplicate rows, and an empty input
    /// returns immediately without issuing invalid `IN ()` SQL.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn find_by_user_ids(
        &self,
        user_ids: &[Uuid],
    ) -> Result<Vec<user_profile_image::Model>, UserProfileImageStoreError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(user_profile_image::Entity::find()
            .filter(user_profile_image::Column::UserId.is_in(user_ids.iter().copied()))
            .all(self.database.as_ref())
            .await?)
    }

    /// Removes the profile image by user ID and returns the deleted row.
    ///
    /// The operation never depends on a detached image's temporary key. A
    /// missing image is an intentional no-op, including concurrent clears.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn clear(
        &self,
        user_id: Uuid,
    ) -> Result<Option<user_profile_image::Model>, UserProfileImageStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            DELETE FROM jellyfin.user_profile_images
            WHERE user_id = $1
            RETURNING user_id, path, last_modified
            ",
            [user_id.into()],
        );
        Ok(user_profile_image::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?)
    }
}
