use sea_orm::{DatabaseConnection, DbBackend, DbErr, EntityTrait, FromQueryResult, Statement};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::server_configuration;

const SERVER_CONFIGURATION_ID: i16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupConfigurationUpdate {
    pub server_name: String,
    pub ui_culture: String,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerConfigurationUpdate {
    pub server_name: String,
    pub ui_culture: String,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
    pub is_startup_wizard_completed: bool,
    pub content_types: Value,
    pub plugin_repositories: Value,
    pub min_resume_pct: i32,
    pub max_resume_pct: i32,
    pub min_resume_duration_seconds: i32,
    pub min_audiobook_resume: i32,
    pub max_audiobook_resume: i32,
    pub allow_client_log_upload: bool,
    pub trickplay_options: Value,
    pub tmdb_api_key: String,
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
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, server_id, tmdb_api_key,
                row_version, created_at, updated_at
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
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, server_id, tmdb_api_key,
                row_version, created_at, updated_at
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
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, tmdb_api_key,
                row_version, created_at, updated_at
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

    /// Enables or disables client diagnostic log uploads.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error. The setting is stored on
    /// the singleton row so API workers observe one `PostgreSQL` source of
    /// truth.
    pub async fn update_client_log_upload(
        &self,
        allow_client_log_upload: bool,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET allow_client_log_upload = $1
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, tmdb_api_key,
                row_version, created_at, updated_at
            ",
            [allow_client_log_upload.into()],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }

    /// Enables or disables network remote access during startup.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error. The boolean is updated
    /// directly on the singleton row to keep startup API workers synchronized
    /// through one `PostgreSQL` source of truth.
    /// Loads or generates a stable server instance identifier.
    ///
    /// Returns the persisted `server_id` if one exists; otherwise generates a
    /// new UUID, persists it, and returns the new value. This ensures the
    /// server identity survives restarts so clients can detect server changes.
    ///
    /// # Errors
    ///
    /// Returns a database error when the read or write fails.
    pub async fn ensure_server_id(&self) -> Result<String, ServerConfigurationStoreError> {
        let model = self.load().await?;
        if !model.server_id.is_empty() {
            return Ok(model.server_id);
        }
        let id = Uuid::new_v4().simple().to_string();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET server_id = $1
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, server_id, tmdb_api_key,
                row_version, created_at, updated_at
            ",
            [id.clone().into()],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)?;
        Ok(id)
    }

    pub async fn update_remote_access(
        &self,
        enable_remote_access: bool,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET enable_remote_access = $1
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, tmdb_api_key,
                row_version, created_at, updated_at
            ",
            [enable_remote_access.into()],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }

    /// Atomically replaces the configured plugin repositories.
    ///
    /// The JSON shape is validated by a `PostgreSQL` array constraint, matching
    /// Jellyfin's configuration-manager behavior of replacing the whole
    /// repository list on save.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error.
    pub async fn update_plugin_repositories(
        &self,
        plugin_repositories: Value,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET plugin_repositories = $1
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, tmdb_api_key,
                row_version, created_at, updated_at
            ",
            [plugin_repositories.into()],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }

    /// Atomically replaces the persisted server-configuration fields.
    ///
    /// Jellyfin's public configuration object is larger than the fields
    /// currently persisted by this Rust port. This method updates every column
    /// represented in the `PostgreSQL` singleton in one statement, preserving
    /// the official replace-on-save behavior for the durable subset and letting
    /// database constraints validate JSON array fields.
    ///
    /// # Errors
    ///
    /// Returns a missing-singleton or database error.
    pub async fn update_server_configuration(
        &self,
        update: ServerConfigurationUpdate,
    ) -> Result<server_configuration::Model, ServerConfigurationStoreError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            UPDATE jellyfin.server_configuration
            SET server_name = $1,
                ui_culture = $2,
                metadata_country_code = $3,
                preferred_metadata_language = $4,
                is_startup_wizard_completed = $5,
                content_types = $6,
                plugin_repositories = $7,
                min_resume_pct = $8,
                max_resume_pct = $9,
                min_resume_duration_seconds = $10,
                min_audiobook_resume = $11,
                max_audiobook_resume = $12,
                allow_client_log_upload = $13,
                trickplay_options = $14,
                tmdb_api_key = $15
            WHERE id = 1
            RETURNING id, server_name, ui_culture, metadata_country_code,
                preferred_metadata_language, is_startup_wizard_completed,
                content_types, plugin_repositories, min_resume_pct, max_resume_pct,
                min_resume_duration_seconds, min_audiobook_resume,
                max_audiobook_resume, allow_client_log_upload, trickplay_options,
                enable_remote_access, tmdb_api_key,
                row_version, created_at, updated_at
            ",
            [
                update.server_name.into(),
                update.ui_culture.into(),
                update.metadata_country_code.into(),
                update.preferred_metadata_language.into(),
                update.is_startup_wizard_completed.into(),
                update.content_types.into(),
                update.plugin_repositories.into(),
                update.min_resume_pct.into(),
                update.max_resume_pct.into(),
                update.min_resume_duration_seconds.into(),
                update.min_audiobook_resume.into(),
                update.max_audiobook_resume.into(),
                update.allow_client_log_upload.into(),
                update.trickplay_options.into(),
                update.tmdb_api_key.into(),
            ],
        );
        server_configuration::Model::find_by_statement(statement)
            .one(&self.database)
            .await?
            .ok_or(ServerConfigurationStoreError::MissingSingleton)
    }
}
