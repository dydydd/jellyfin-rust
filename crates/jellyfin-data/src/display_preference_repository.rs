use chrono::Utc;
use sea_orm::{
    ActiveValue::NotSet, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
    sea_query::OnConflict,
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::display_preference;

#[derive(Debug, Error)]
pub enum DisplayPreferenceStoreError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("{field} is too long: {actual} > {max}")]
    FieldTooLong {
        field: &'static str,
        actual: usize,
        max: usize,
    },
    #[error("display preferences must be a JSON object")]
    InvalidPreferences,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct DisplayPreferenceRepository {
    database: DatabaseConnection,
}

impl DisplayPreferenceRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Finds stored display preferences for a user/item/client tuple.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an invalid client name, or a database
    /// error when the query fails.
    pub async fn find(
        &self,
        user_id: Uuid,
        item_id: Uuid,
        client: &str,
    ) -> Result<Option<display_preference::Model>, DisplayPreferenceStoreError> {
        validate_client(client)?;
        Ok(display_preference::Entity::find()
            .filter(display_preference::Column::UserId.eq(user_id))
            .filter(display_preference::Column::ItemId.eq(item_id))
            .filter(display_preference::Column::Client.eq(client))
            .one(&self.database)
            .await?)
    }

    /// Inserts or atomically replaces stored display preferences.
    ///
    /// This uses `PostgreSQL`'s `(user_id, item_id, client)` unique index as
    /// the upsert conflict target so repeated client saves are atomic and keep
    /// the row stable.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an invalid client or JSON payload, or a
    /// database error when the upsert fails.
    pub async fn upsert(
        &self,
        user_id: Uuid,
        item_id: Uuid,
        client: &str,
        preferences: Value,
    ) -> Result<display_preference::Model, DisplayPreferenceStoreError> {
        validate_client(client)?;
        if !preferences.is_object() {
            return Err(DisplayPreferenceStoreError::InvalidPreferences);
        }
        let now = Utc::now();
        Ok(
            display_preference::Entity::insert(display_preference::ActiveModel {
                id: NotSet,
                user_id: Set(user_id),
                item_id: Set(item_id),
                client: Set(client.to_owned()),
                preferences: Set(preferences),
                created_at: Set(now),
                updated_at: Set(now),
            })
            .on_conflict(
                OnConflict::columns([
                    display_preference::Column::UserId,
                    display_preference::Column::ItemId,
                    display_preference::Column::Client,
                ])
                .update_columns([
                    display_preference::Column::Preferences,
                    display_preference::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec_with_returning(&self.database)
            .await?,
        )
    }
}

fn validate_client(client: &str) -> Result<(), DisplayPreferenceStoreError> {
    let len = client.chars().count();
    if client.is_empty() {
        return Err(DisplayPreferenceStoreError::EmptyField("client"));
    }
    if len > 128 {
        return Err(DisplayPreferenceStoreError::FieldTooLong {
            field: "client",
            actual: len,
            max: 128,
        });
    }
    Ok(())
}
