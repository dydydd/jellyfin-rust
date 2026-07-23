use chrono::{DateTime, Utc};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, DbErr, DeleteResult,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, sea_query::Expr,
};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{api_key, device, device_option};

#[derive(Debug, Error)]
pub enum AuthenticationStoreError {
    #[error("authentication {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("authentication {field} exceeds its {max} character limit")]
    FieldTooLong { field: &'static str, max: usize },
    #[error("device capabilities must be a JSON object")]
    InvalidCapabilities,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// Persistence operations for server API keys.
#[derive(Clone)]
pub struct ApiKeyRepository {
    database: DatabaseConnection,
}

impl ApiKeyRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Creates a key and returns its recoverable access token.
    ///
    /// Jellyfin's administrative API lists existing token values, so this
    /// persistence contract cannot use a one-way digest without an upstream
    /// API change to one-time token display.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an empty or oversized name, or a
    /// database error when insertion fails.
    pub async fn create(&self, name: &str) -> Result<api_key::Model, AuthenticationStoreError> {
        validate_required("API key name", name, 64)?;
        let now = Utc::now();
        Ok(api_key::ActiveModel {
            id: NotSet,
            date_created: Set(now),
            date_last_activity: Set(now),
            name: Set(name.to_owned()),
            access_token: Set(new_access_token()),
        }
        .insert(&self.database)
        .await?)
    }

    /// Lists API keys in stable creation order.
    ///
    /// # Errors
    ///
    /// Returns a database error when loading keys fails.
    pub async fn list(&self) -> Result<Vec<api_key::Model>, AuthenticationStoreError> {
        Ok(api_key::Entity::find()
            .order_by_asc(api_key::Column::Id)
            .all(&self.database)
            .await?)
    }

    /// Finds an API key by its exact token.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn find_by_token(
        &self,
        token: &str,
    ) -> Result<Option<api_key::Model>, AuthenticationStoreError> {
        Ok(api_key::Entity::find()
            .filter(api_key::Column::AccessToken.eq(token))
            .one(&self.database)
            .await?)
    }

    /// Updates the key's last-activity timestamp.
    ///
    /// # Errors
    ///
    /// Returns a database error when updating fails.
    pub async fn touch(
        &self,
        token: &str,
        at: DateTime<Utc>,
    ) -> Result<u64, AuthenticationStoreError> {
        Ok(api_key::Entity::update_many()
            .col_expr(api_key::Column::DateLastActivity, Expr::value(at))
            .filter(api_key::Column::AccessToken.eq(token))
            .exec(&self.database)
            .await?
            .rows_affected)
    }

    /// Revokes an API key by exact token.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn revoke(&self, token: &str) -> Result<u64, AuthenticationStoreError> {
        let DeleteResult { rows_affected } = api_key::Entity::delete_many()
            .filter(api_key::Column::AccessToken.eq(token))
            .exec(&self.database)
            .await?;
        Ok(rows_affected)
    }
}

/// Data required to create a device authentication record.
#[derive(Debug, Clone)]
pub struct NewDevice {
    pub user_id: Uuid,
    pub app_name: String,
    pub app_version: String,
    pub device_name: String,
    pub device_id: String,
}

impl NewDevice {
    #[must_use]
    pub fn new(
        user_id: Uuid,
        app_name: impl Into<String>,
        app_version: impl Into<String>,
        device_name: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            user_id,
            app_name: app_name.into(),
            app_version: app_version.into(),
            device_name: device_name.into(),
            device_id: device_id.into(),
        }
    }
}

/// Filters and pagination used by Jellyfin device lookups.
#[derive(Debug, Clone, Default)]
pub struct DeviceQuery {
    pub skip: Option<u64>,
    pub limit: Option<u64>,
    pub user_id: Option<Uuid>,
    pub device_id: Option<String>,
    pub access_token: Option<String>,
    pub is_active: Option<bool>,
    pub active_since: Option<DateTime<Utc>>,
}

/// A page of devices and the unpaged match count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePage {
    pub start_index: Option<u64>,
    pub total_record_count: u64,
    pub items: Vec<device::Model>,
}

/// Persistence and query operations for authenticated devices.
#[derive(Clone)]
pub struct DeviceRepository {
    database: DatabaseConnection,
}

impl DeviceRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Creates a device with a new recoverable access token.
    ///
    /// Device authentication and logout both require exact token lookup, and
    /// login responses must return the token. A digest-only representation is
    /// therefore incompatible with the current upstream contract.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid metadata, or a database error
    /// when insertion fails.
    pub async fn create(
        &self,
        new_device: NewDevice,
    ) -> Result<device::Model, AuthenticationStoreError> {
        self.insert(new_device, false).await
    }

    /// Creates an active device session with a new recoverable access token.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid metadata, or a database error
    /// when insertion fails.
    pub async fn create_session(
        &self,
        new_device: NewDevice,
    ) -> Result<device::Model, AuthenticationStoreError> {
        self.insert(new_device, true).await
    }

    async fn insert(
        &self,
        new_device: NewDevice,
        is_active: bool,
    ) -> Result<device::Model, AuthenticationStoreError> {
        Self::insert_with(&self.database, new_device, is_active).await
    }

    pub(crate) async fn insert_with<C: sea_orm::ConnectionTrait>(
        connection: &C,
        new_device: NewDevice,
        is_active: bool,
    ) -> Result<device::Model, AuthenticationStoreError> {
        validate_device(&new_device)?;
        let now = Utc::now();
        Ok(device::ActiveModel {
            id: NotSet,
            user_id: Set(new_device.user_id),
            access_token: Set(new_access_token()),
            app_name: Set(new_device.app_name),
            app_version: Set(new_device.app_version),
            device_name: Set(new_device.device_name),
            device_id: Set(new_device.device_id),
            is_active: Set(is_active),
            capabilities: Set(json!({})),
            date_created: Set(now),
            date_modified: Set(now),
            date_last_activity: Set(now),
        }
        .insert(connection)
        .await?)
    }

    /// Finds a device session by its exact access token.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn find_by_token(
        &self,
        access_token: &str,
    ) -> Result<Option<device::Model>, AuthenticationStoreError> {
        Ok(device::Entity::find()
            .filter(device::Column::AccessToken.eq(access_token))
            .one(&self.database)
            .await?)
    }

    /// Revokes all device sessions for a user except an optional current
    /// access token.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn revoke_user_tokens(
        &self,
        user_id: Uuid,
        except_access_token: Option<&str>,
    ) -> Result<u64, AuthenticationStoreError> {
        let mut devices = device::Entity::delete_many().filter(device::Column::UserId.eq(user_id));
        if let Some(token) = except_access_token {
            devices = devices.filter(device::Column::AccessToken.ne(token));
        }
        Ok(devices.exec(&self.database).await?.rows_affected)
    }

    /// Deletes a device authentication record by exact access token.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete_by_token(
        &self,
        access_token: &str,
    ) -> Result<u64, AuthenticationStoreError> {
        Ok(device::Entity::delete_many()
            .filter(device::Column::AccessToken.eq(access_token))
            .exec(&self.database)
            .await?
            .rows_affected)
    }

    /// Deletes every authentication record for an exact device identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete_by_device_id(
        &self,
        device_id: &str,
    ) -> Result<u64, AuthenticationStoreError> {
        Ok(device::Entity::delete_many()
            .filter(device::Column::DeviceId.eq(device_id))
            .exec(&self.database)
            .await?
            .rows_affected)
    }

    /// Returns devices matching the supplied exact filters.
    ///
    /// A limit of zero matches upstream behavior and means no limit.
    ///
    /// # Errors
    ///
    /// Returns a database error when counting or loading devices fails.
    pub async fn query(&self, query: &DeviceQuery) -> Result<DevicePage, AuthenticationStoreError> {
        let mut devices = device::Entity::find();
        if let Some(user_id) = query.user_id {
            devices = devices.filter(device::Column::UserId.eq(user_id));
        }
        if let Some(device_id) = query.device_id.as_deref() {
            devices = devices.filter(Expr::cust_with_values(
                "lower(device_id) = lower($1::text)",
                [device_id.to_owned()],
            ));
        }
        if let Some(access_token) = query.access_token.as_deref() {
            devices = devices.filter(device::Column::AccessToken.eq(access_token));
        }
        if let Some(is_active) = query.is_active {
            devices = devices.filter(device::Column::IsActive.eq(is_active));
        }
        if let Some(active_since) = query.active_since {
            devices = devices.filter(device::Column::DateLastActivity.gte(active_since));
        }

        let total_record_count = devices.clone().count(&self.database).await?;
        devices = devices
            .order_by_asc(device::Column::Id)
            .offset(query.skip.unwrap_or(0));
        if let Some(limit) = query.limit.filter(|limit| *limit > 0) {
            devices = devices.limit(limit);
        }
        let items = devices.all(&self.database).await?;

        Ok(DevicePage {
            start_index: query.skip,
            total_record_count,
            items,
        })
    }

    /// Returns the most recently active record for a device identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn latest_by_device_id(
        &self,
        device_id: &str,
    ) -> Result<Option<device::Model>, AuthenticationStoreError> {
        Ok(device::Entity::find()
            .filter(device::Column::DeviceId.eq(device_id))
            .order_by_desc(device::Column::DateLastActivity)
            .one(&self.database)
            .await?)
    }

    /// Saves updated device metadata and refreshes `date_modified`.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid metadata, or a database error
    /// when updating fails.
    pub async fn update(
        &self,
        model: device::Model,
    ) -> Result<device::Model, AuthenticationStoreError> {
        validate_device_model(&model)?;
        let active = device::ActiveModel {
            id: Set(model.id),
            user_id: Set(model.user_id),
            access_token: Set(model.access_token),
            app_name: Set(model.app_name),
            app_version: Set(model.app_version),
            device_name: Set(model.device_name),
            device_id: Set(model.device_id),
            is_active: Set(model.is_active),
            capabilities: Set(model.capabilities),
            date_created: Set(model.date_created),
            date_modified: Set(Utc::now()),
            date_last_activity: Set(model.date_last_activity),
        };
        Ok(active.update(&self.database).await?)
    }

    /// Persists client capabilities for a device session identified by token.
    ///
    /// # Errors
    ///
    /// Returns a validation error for non-object capabilities, or a database
    /// error when updating fails.
    pub async fn update_capabilities_by_token(
        &self,
        access_token: &str,
        capabilities: Value,
    ) -> Result<u64, AuthenticationStoreError> {
        if !capabilities.is_object() {
            return Err(AuthenticationStoreError::InvalidCapabilities);
        }
        Ok(device::Entity::update_many()
            .col_expr(device::Column::Capabilities, Expr::value(capabilities))
            .col_expr(device::Column::DateModified, Expr::value(Utc::now()))
            .filter(device::Column::AccessToken.eq(access_token))
            .exec(&self.database)
            .await?
            .rows_affected)
    }

    /// Deletes a device authentication record.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete(&self, id: i64) -> Result<u64, AuthenticationStoreError> {
        Ok(device::Entity::delete_by_id(id)
            .exec(&self.database)
            .await?
            .rows_affected)
    }
}

/// Persistence operations for custom device options.
#[derive(Clone)]
pub struct DeviceOptionsRepository {
    database: DatabaseConnection,
}

impl DeviceOptionsRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Finds custom options for an exact device identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn get(
        &self,
        device_id: &str,
    ) -> Result<Option<device_option::Model>, AuthenticationStoreError> {
        Ok(device_option::Entity::find()
            .filter(device_option::Column::DeviceId.eq(device_id))
            .one(&self.database)
            .await?)
    }

    /// Finds custom options for exact device identifiers.
    ///
    /// # Errors
    ///
    /// Returns a database error when lookup fails.
    pub async fn find_by_device_ids(
        &self,
        device_ids: &[String],
    ) -> Result<Vec<device_option::Model>, AuthenticationStoreError> {
        if device_ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(device_option::Entity::find()
            .filter(device_option::Column::DeviceId.is_in(device_ids.iter().cloned()))
            .all(&self.database)
            .await?)
    }

    /// Inserts or updates custom options for a device identifier.
    ///
    /// This uses `PostgreSQL`'s unique `device_id` index as the conflict target
    /// so repeated writes are atomic and keep the stable options row id.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an invalid device id, or a database error
    /// when the upsert fails.
    pub async fn upsert_custom_name(
        &self,
        device_id: &str,
        custom_name: Option<String>,
    ) -> Result<device_option::Model, AuthenticationStoreError> {
        validate_required("device id", device_id, 256)?;
        Ok(device_option::Entity::insert(device_option::ActiveModel {
            id: NotSet,
            device_id: Set(device_id.to_owned()),
            custom_name: Set(custom_name),
        })
        .on_conflict(
            OnConflict::column(device_option::Column::DeviceId)
                .update_column(device_option::Column::CustomName)
                .to_owned(),
        )
        .exec_with_returning(&self.database)
        .await?)
    }
}

fn new_access_token() -> String {
    Uuid::new_v4().simple().to_string()
}

fn validate_device(device: &NewDevice) -> Result<(), AuthenticationStoreError> {
    validate_length("app name", &device.app_name, 64)?;
    validate_length("app version", &device.app_version, 32)?;
    validate_length("device name", &device.device_name, 64)?;
    validate_required("device id", &device.device_id, 256)
}

fn validate_device_model(device: &device::Model) -> Result<(), AuthenticationStoreError> {
    validate_length("app name", &device.app_name, 64)?;
    validate_length("app version", &device.app_version, 32)?;
    validate_length("device name", &device.device_name, 64)?;
    validate_required("device id", &device.device_id, 256)
}

fn validate_required(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), AuthenticationStoreError> {
    if value.is_empty() {
        return Err(AuthenticationStoreError::EmptyField(field));
    }
    validate_length(field, value, max)
}

fn validate_length(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), AuthenticationStoreError> {
    if value.chars().count() > max {
        return Err(AuthenticationStoreError::FieldTooLong { field, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_authentication_metadata() {
        assert!(matches!(
            validate_required("API key name", "", 64),
            Err(AuthenticationStoreError::EmptyField("API key name"))
        ));
        assert!(matches!(
            validate_required("device id", &"x".repeat(257), 256),
            Err(AuthenticationStoreError::FieldTooLong {
                field: "device id",
                max: 256
            })
        ));
    }
}
