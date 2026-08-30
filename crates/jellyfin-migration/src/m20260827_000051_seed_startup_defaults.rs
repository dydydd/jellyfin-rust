use sea_orm_migration::prelude::*;

use crate::startup_routines::StartupMigrationRunner;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                CREATE TABLE IF NOT EXISTS jellyfin.startup_migrations (
                    routine_id text PRIMARY KEY,
                    applied_at timestamptz NOT NULL DEFAULT clock_timestamp()
                );

                ALTER TABLE jellyfin.users
                    ADD COLUMN IF NOT EXISTS enable_local_password boolean NOT NULL DEFAULT false,
                    ADD COLUMN IF NOT EXISTS invalid_login_attempt_count integer NOT NULL DEFAULT 0,
                    ADD COLUMN IF NOT EXISTS login_attempts_before_lockout integer NOT NULL DEFAULT -1;

                UPDATE jellyfin.users
                SET enable_local_password = COALESCE(
                        (preferences ->> 'EnableLocalPassword')::boolean,
                        false
                    ),
                    invalid_login_attempt_count = COALESCE(
                        NULLIF(policy ->> 'InvalidLoginAttemptCount', '')::integer,
                        0
                    ),
                    login_attempts_before_lockout = COALESCE(
                        NULLIF(policy ->> 'LoginAttemptsBeforeLockout', '')::integer,
                        -1
                    );

                ALTER TABLE jellyfin.server_configuration
                    ADD COLUMN IF NOT EXISTS cast_receiver_applications jsonb NOT NULL DEFAULT '[]'::jsonb;

                UPDATE jellyfin.server_configuration
                SET plugin_repositories = CASE
                        WHEN plugin_repositories = '[]'::jsonb OR plugin_repositories IS NULL
                            THEN jsonb_build_array(jsonb_build_object(
                                'Name', 'Jellyfin Stable',
                                'Url', 'https://repo.jellyfin.org/files/plugin/manifest.json',
                                'Enabled', true
                            ))
                        ELSE (
                            SELECT jsonb_agg(
                                CASE
                                    WHEN lower(value ->> 'Url') =
                                         'https://repo.jellyfin.org/releases/plugin/manifest-stable.json'
                                        THEN jsonb_set(value, '{Url}',
                                            to_jsonb('https://repo.jellyfin.org/files/plugin/manifest.json'::text))
                                    ELSE value
                                END
                            )
                            FROM jsonb_array_elements(plugin_repositories) AS value
                        )
                    END
                WHERE plugin_repositories = '[]'::jsonb
                   OR plugin_repositories IS NULL
                   OR EXISTS (
                        SELECT 1
                        FROM jsonb_array_elements(plugin_repositories) AS value
                        WHERE lower(value ->> 'Url') =
                              'https://repo.jellyfin.org/releases/plugin/manifest-stable.json'
                   );

                UPDATE jellyfin.server_configuration
                SET cast_receiver_applications = jsonb_build_array(
                        jsonb_build_object('Id', 'F007D354', 'Name', 'Stable'),
                        jsonb_build_object('Id', '6F511C87', 'Name', 'Unstable')
                    )
                WHERE cast_receiver_applications = '[]'::jsonb
                   OR cast_receiver_applications IS NULL;
                ",
            )
            .await?;

        StartupMigrationRunner::run(manager, false).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                ALTER TABLE jellyfin.users
                    DROP COLUMN IF EXISTS enable_local_password,
                    DROP COLUMN IF EXISTS invalid_login_attempt_count,
                    DROP COLUMN IF EXISTS login_attempts_before_lockout;
                ALTER TABLE jellyfin.server_configuration
                    DROP COLUMN IF EXISTS cast_receiver_applications;
                DROP TABLE IF EXISTS jellyfin.startup_migrations;
                ",
            )
            .await?;
        Ok(())
    }
}
