use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, DbErr, DeleteResult,
    EntityTrait, IntoActiveModel, ModelTrait, QueryFilter, QuerySelect, Set, SqlErr,
    TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use crate::authentication_repository::{AuthenticationStoreError, DeviceRepository, NewDevice};
use crate::entities::{device, quick_connect};

/// Fields persisted when a device starts Quick Connect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewQuickConnectRequest {
    pub code: String,
    pub secret: String,
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Authorized request and the active device session created for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedQuickConnect {
    pub request: quick_connect::Model,
    pub device: device::Model,
}

/// Quick Connect persistence failure.
#[derive(Debug, Error)]
pub enum QuickConnectStoreError {
    #[error("Quick Connect {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("Quick Connect {field} exceeds its {max} character limit")]
    FieldTooLong { field: &'static str, max: usize },
    #[error("Quick Connect code must contain exactly six ASCII digits")]
    InvalidCode,
    #[error("Quick Connect secret must contain exactly 64 hexadecimal characters")]
    InvalidSecret,
    #[error("Quick Connect expiration must be after creation")]
    InvalidExpiration,
    #[error("Quick Connect code or secret already exists")]
    Conflict,
    #[error("Quick Connect request was not found or has expired")]
    NotFound,
    #[error("Quick Connect request is already authorized")]
    AlreadyAuthorized,
    #[error(transparent)]
    Device(#[from] AuthenticationStoreError),
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed Quick Connect state transitions.
#[derive(Clone)]
pub struct QuickConnectRepository {
    database: DatabaseConnection,
}

impl QuickConnectRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Persists a pending request.
    ///
    /// # Errors
    ///
    /// Returns a validation, uniqueness-conflict, or database error.
    pub async fn create(
        &self,
        request: NewQuickConnectRequest,
    ) -> Result<quick_connect::Model, QuickConnectStoreError> {
        validate_request(&request)?;
        quick_connect::ActiveModel {
            id: NotSet,
            code: Set(request.code),
            secret: Set(request.secret),
            device_id: Set(request.device_id),
            device_name: Set(request.device_name),
            app_name: Set(request.app_name),
            app_version: Set(request.app_version),
            created_at: Set(request.created_at),
            expires_at: Set(request.expires_at),
            authorized_at: Set(None),
            user_id: Set(None),
            authorized_device_id: Set(None),
        }
        .insert(&self.database)
        .await
        .map_err(map_insert_error)
    }

    /// Loads an unexpired request by its exact secret.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn status(
        &self,
        secret: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<quick_connect::Model>, QuickConnectStoreError> {
        Ok(quick_connect::Entity::find()
            .filter(quick_connect::Column::Secret.eq(secret))
            .filter(quick_connect::Column::ExpiresAt.gt(now))
            .one(&self.database)
            .await?)
    }

    /// Authorizes one request and creates its active device session atomically.
    ///
    /// A `SELECT FOR UPDATE` serializes competing authorizations for the same
    /// code. The request state and device token commit in one transaction.
    ///
    /// # Errors
    ///
    /// Returns not-found for unknown/expired codes, already-authorized for a
    /// completed request, or a device/database error.
    pub async fn authorize(
        &self,
        code: &str,
        user_id: Uuid,
        now: DateTime<Utc>,
        authorized_expires_at: DateTime<Utc>,
    ) -> Result<AuthorizedQuickConnect, QuickConnectStoreError> {
        if authorized_expires_at <= now {
            return Err(QuickConnectStoreError::InvalidExpiration);
        }
        let transaction = self.database.begin().await?;
        let Some(request) = quick_connect::Entity::find()
            .filter(quick_connect::Column::Code.eq(code))
            .lock_exclusive()
            .one(&transaction)
            .await?
        else {
            return Err(QuickConnectStoreError::NotFound);
        };
        if request.expires_at <= now {
            request.delete(&transaction).await?;
            transaction.commit().await?;
            return Err(QuickConnectStoreError::NotFound);
        }
        if request.authorized_at.is_some() {
            return Err(QuickConnectStoreError::AlreadyAuthorized);
        }

        let device = DeviceRepository::insert_with(
            &transaction,
            NewDevice::new(
                user_id,
                &request.app_name,
                &request.app_version,
                &request.device_name,
                &request.device_id,
            ),
            true,
        )
        .await?;
        let mut active = request.into_active_model();
        active.authorized_at = Set(Some(now));
        active.expires_at = Set(authorized_expires_at);
        active.user_id = Set(Some(user_id));
        active.authorized_device_id = Set(Some(device.id));
        let request = active.update(&transaction).await?;
        transaction.commit().await?;
        Ok(AuthorizedQuickConnect { request, device })
    }

    /// Loads the active device session associated with an authorized secret.
    ///
    /// # Errors
    ///
    /// Returns not-found for unknown, pending, expired, or inconsistent rows,
    /// and a database error when lookup fails.
    pub async fn authorized(
        &self,
        secret: &str,
        now: DateTime<Utc>,
    ) -> Result<AuthorizedQuickConnect, QuickConnectStoreError> {
        let request = quick_connect::Entity::find()
            .filter(quick_connect::Column::Secret.eq(secret))
            .filter(quick_connect::Column::ExpiresAt.gt(now))
            .filter(quick_connect::Column::AuthorizedAt.is_not_null())
            .one(&self.database)
            .await?
            .ok_or(QuickConnectStoreError::NotFound)?;
        let device_id = request
            .authorized_device_id
            .ok_or(QuickConnectStoreError::NotFound)?;
        let device = device::Entity::find_by_id(device_id)
            .one(&self.database)
            .await?
            .ok_or(QuickConnectStoreError::NotFound)?;
        Ok(AuthorizedQuickConnect { request, device })
    }

    /// Deletes requests whose indexed expiry is at or before `now`.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn purge_expired(&self, now: DateTime<Utc>) -> Result<u64, QuickConnectStoreError> {
        let DeleteResult { rows_affected } = quick_connect::Entity::delete_many()
            .filter(quick_connect::Column::ExpiresAt.lte(now))
            .exec(&self.database)
            .await?;
        Ok(rows_affected)
    }
}

fn validate_request(request: &NewQuickConnectRequest) -> Result<(), QuickConnectStoreError> {
    if request.code.len() != 6 || !request.code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(QuickConnectStoreError::InvalidCode);
    }
    if request.secret.len() != 64 || !request.secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(QuickConnectStoreError::InvalidSecret);
    }
    validate_required("device id", &request.device_id, 256)?;
    validate_required("device name", &request.device_name, 64)?;
    validate_required("app name", &request.app_name, 64)?;
    validate_required("app version", &request.app_version, 32)?;
    if request.expires_at <= request.created_at {
        return Err(QuickConnectStoreError::InvalidExpiration);
    }
    Ok(())
}

fn validate_required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), QuickConnectStoreError> {
    if value.is_empty() {
        return Err(QuickConnectStoreError::EmptyField(field));
    }
    if value.chars().count() > max {
        return Err(QuickConnectStoreError::FieldTooLong { field, max });
    }
    Ok(())
}

fn map_insert_error(error: DbErr) -> QuickConnectStoreError {
    if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        QuickConnectStoreError::Conflict
    } else {
        QuickConnectStoreError::Database(error)
    }
}
