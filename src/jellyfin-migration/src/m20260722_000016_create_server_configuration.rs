use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                CREATE TABLE IF NOT EXISTS jellyfin.server_configuration (
                    id smallint PRIMARY KEY,
                    server_name text NOT NULL,
                    ui_culture text NOT NULL DEFAULT '',
                    metadata_country_code text NOT NULL DEFAULT '',
                    preferred_metadata_language text NOT NULL DEFAULT '',
                    is_startup_wizard_completed boolean NOT NULL DEFAULT false,
                    row_version bigint NOT NULL DEFAULT 1,
                    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    CONSTRAINT server_configuration_singleton CHECK (id = 1)
                );

                DROP TRIGGER IF EXISTS server_configuration_touch_row_version
                    ON jellyfin.server_configuration;
                CREATE TRIGGER server_configuration_touch_row_version
                    BEFORE UPDATE ON jellyfin.server_configuration
                    FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_row_version();

                INSERT INTO jellyfin.server_configuration (
                    id, server_name, ui_culture, metadata_country_code,
                    preferred_metadata_language, is_startup_wizard_completed
                )
                VALUES (1, 'Jellyfin', '', '', '', false)
                ON CONFLICT (id) DO NOTHING;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.server_configuration CASCADE;")
            .await?;
        Ok(())
    }
}
