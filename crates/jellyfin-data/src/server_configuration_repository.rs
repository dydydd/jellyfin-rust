use sea_orm::{DatabaseConnection, DbBackend, DbErr, EntityTrait, FromQueryResult, Statement};
use thiserror::Error;

use crate::entities::server_configuration;

const SERVER_CONFIGURATION_ID: i16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupConfigurationUpdate {
    pub server_name: String,
    pub ui_culture: String,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
}

#[derive(Debug, Error)]
pub enum ServerConfigurationStoreError {
    #[error("the server configuration singleton is missing")]
    MissingSingleton,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed storage for the singleton server configuration.
#[derive(Clone)]
pub struct ServerConfigurationRepository {
    database: DatabaseConnection,
}

impl ServerConfigurationRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Loads the singleton row seeded by the server-configuration migration.
    ///
    /// # Errors
    ///
    /// Returns [`ServerConfigurationStoreError::MissingSingleton`] when the
    /// seeded row was removed, or a database error when `PostgreSQL` fails.
    pub async fn load(&self) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        server_configuration::Entity::find_by_id(SERVER_CONFIGURATION_ID)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }

    /// Atomically replaces the four startup-wizard configuration fields.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error. No partial update is
    /// visible because `PostgreSQL` executes the update as one statement.
    pub async fn update_startup_configuration(
        &self,
        update: StartupConfigurationUpdate,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET server_name = $1,
                ui_culture = $2,
                metadata_country_code = $3,
                preferred_metadata_language = $4
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, row_version, created_at, updated_at
            ",
            [
                update.server_name.into(),
                update.ui_culture.into(),
                update.metadata_country_code.into(),
                update.preferred_metadata_language.into(),
            ],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }

    /// Marks the startup wizard complete and returns the committed row.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error. Repeated calls are
    /// intentionally valid and remain a single atomic `PostgreSQL` update.
    pub async fn complete_startup(
        &self,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_string(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET is_startup_wizard_completed = true
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, row_version, created_at, updated_at
            "
            .to_owned(),
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }

    /// Atomically replaces or removes the content-type override for one path.
    ///
    /// Empty and whitespace-only values remove the override. `PostgreSQL`
    /// filters malformed blank-name entries and compares paths without case
    /// sensitivity in the same row update. Concurrent updates for different
    /// paths therefore compose instead of replacing the complete JSON array.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error.
    pub async fn update_content_type_override(
        &self,
        path: &str,
        content_type: Option<&str>,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration AS configuration
            SET content_types = (
                SELECT
                    COALESCE(
                        jsonb_agg(entry.element ORDER BY entry.ordinality)
                            FILTER (
                                WHERE entry.element ->> 'Name' IS NOT NULL
                                  AND entry.element ->> 'Name' !~ '^[[:space:]]*$'
                                  AND lower(entry.element ->> 'Name') <> lower($1::text)
                            ),
                        '[]'::jsonb
                    )
                    || CASE
                        WHEN $2::text IS NULL OR $2::text ~ '^[[:space:]]*$'
                            THEN '[]'::jsonb
                        ELSE jsonb_build_array(
                            jsonb_build_object('Name', $1::text, 'Value', $2::text)
                        )
                    END
                FROM jsonb_array_elements(configuration.content_types)
                    WITH ORDINALITY AS entry(element, ordinality)
            )
            WHERE configuration.id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, row_version, created_at, updated_at
            ",
            [
                path.to_owned().into(),
                content_type.map(str::to_owned).into(),
            ],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }
}
