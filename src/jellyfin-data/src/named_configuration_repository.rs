use sea_orm::{DbBackend, DbErr, EntityTrait, FromQueryResult, Statement};
use serde_json::Value;
use thiserror::Error;

use crate::entities::named_configuration;

#[derive(Debug, Error)]
pub enum NamedConfigurationStoreError {
    #[error("named configuration key must not be blank")]
    BlankKey,
    #[error("named configuration {0} was not found")]
    NotFound(String),
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct NamedConfigurationRepository {
    database: crate::SharedDatabase,
}

impl NamedConfigurationRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Loads a named configuration by canonical key.
    ///
    /// # Errors
    ///
    /// Returns [`NamedConfigurationStoreError::BlankKey`] for blank keys,
    /// [`NamedConfigurationStoreError::NotFound`] when no row exists, or a
    /// database error.
    pub async fn load(
        &self,
        key: &str,
    ) -> Result<named_configuration::Model, NamedConfigurationStoreError> {
        let canonical = canonical_key(key)?;
        if let Some(configuration) = named_configuration::Entity::find_by_id(canonical)
            .one(self.database.as_ref())
            .await?
        {
            Ok(configuration)
        } else {
            Err(NamedConfigurationStoreError::NotFound(canonical_key(key)?))
        }
    }

    /// Inserts or atomically replaces a named configuration object.
    ///
    /// `PostgreSQL` enforces that the stored document is a JSON object. The
    /// `ON CONFLICT` upsert mirrors Jellyfin's save-by-key configuration
    /// manager behavior and lets concurrent writers converge on one row.
    ///
    /// # Errors
    ///
    /// Returns a blank-key or database error.
    pub async fn save(
        &self,
        key: &str,
        configuration: Value,
    ) -> Result<named_configuration::Model, NamedConfigurationStoreError> {
        let key = canonical_key(key)?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            INSERT INTO jellyfin.named_configurations (key, configuration)
            VALUES ($1, $2)
            ON CONFLICT (key) DO UPDATE
                SET configuration = EXCLUDED.configuration
            RETURNING key, configuration, row_version, created_at, updated_at
            ",
            [key.into(), configuration.into()],
        );
        named_configuration::Model::find_by_statement(statement)
            .one(self.database.as_ref())
            .await?
            .ok_or_else(|| NamedConfigurationStoreError::NotFound("<upsert>".to_owned()))
    }
}

fn canonical_key(key: &str) -> Result<String, NamedConfigurationStoreError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(NamedConfigurationStoreError::BlankKey);
    }
    Ok(key.to_ascii_lowercase())
}
