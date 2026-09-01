use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE jellyfin.server_configuration
                    ADD COLUMN IF NOT EXISTS log_file_retention_days integer NOT NULL DEFAULT 3,
                    ADD COLUMN IF NOT EXISTS enable_metrics boolean NOT NULL DEFAULT false,
                    ADD COLUMN IF NOT EXISTS enable_normalized_item_by_name_ids boolean NOT NULL DEFAULT true,
                    ADD COLUMN IF NOT EXISTS metadata_path text NOT NULL DEFAULT '',
                    ADD COLUMN IF NOT EXISTS sort_replace_characters jsonb
                        NOT NULL DEFAULT '[".", "+", "%"]'::jsonb,
                    ADD COLUMN IF NOT EXISTS sort_remove_characters jsonb
                        NOT NULL DEFAULT '["''", "&", "-", "{", "}", "''"]'::jsonb,
                    ADD COLUMN IF NOT EXISTS sort_remove_words jsonb
                        NOT NULL DEFAULT '["the", "a", "an"]'::jsonb,
                    ADD COLUMN IF NOT EXISTS inactive_session_threshold integer NOT NULL DEFAULT 0,
                    ADD COLUMN IF NOT EXISTS library_monitor_delay integer NOT NULL DEFAULT 60,
                    ADD COLUMN IF NOT EXISTS library_update_duration integer NOT NULL DEFAULT 30,
                    ADD COLUMN IF NOT EXISTS cache_size integer,
                    ADD COLUMN IF NOT EXISTS image_saving_convention smallint NOT NULL DEFAULT 0,
                    ADD COLUMN IF NOT EXISTS save_metadata_hidden boolean NOT NULL DEFAULT false,
                    ADD COLUMN IF NOT EXISTS remote_client_bitrate_limit integer NOT NULL DEFAULT 0,
                    ADD COLUMN IF NOT EXISTS enable_folder_view boolean NOT NULL DEFAULT false,
                    ADD COLUMN IF NOT EXISTS enable_grouping_movies_into_collections boolean NOT NULL DEFAULT false,
                    ADD COLUMN IF NOT EXISTS enable_grouping_shows_into_collections boolean NOT NULL DEFAULT false,
                    ADD COLUMN IF NOT EXISTS display_specials_within_seasons boolean NOT NULL DEFAULT true,
                    ADD COLUMN IF NOT EXISTS enable_external_content_in_suggestions boolean NOT NULL DEFAULT true,
                    ADD COLUMN IF NOT EXISTS cors_hosts jsonb NOT NULL DEFAULT '["*"]'::jsonb,
                    ADD COLUMN IF NOT EXISTS activity_log_retention_days integer DEFAULT 30,
                    ADD COLUMN IF NOT EXISTS library_scan_fanout_concurrency integer NOT NULL DEFAULT 0,
                    ADD COLUMN IF NOT EXISTS library_metadata_refresh_concurrency integer NOT NULL DEFAULT 0;
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE jellyfin.server_configuration
                    DROP COLUMN IF EXISTS log_file_retention_days,
                    DROP COLUMN IF EXISTS enable_metrics,
                    DROP COLUMN IF EXISTS enable_normalized_item_by_name_ids,
                    DROP COLUMN IF EXISTS metadata_path,
                    DROP COLUMN IF EXISTS sort_replace_characters,
                    DROP COLUMN IF EXISTS sort_remove_characters,
                    DROP COLUMN IF EXISTS sort_remove_words,
                    DROP COLUMN IF EXISTS inactive_session_threshold,
                    DROP COLUMN IF EXISTS library_monitor_delay,
                    DROP COLUMN IF EXISTS library_update_duration,
                    DROP COLUMN IF EXISTS cache_size,
                    DROP COLUMN IF EXISTS image_saving_convention,
                    DROP COLUMN IF EXISTS save_metadata_hidden,
                    DROP COLUMN IF EXISTS remote_client_bitrate_limit,
                    DROP COLUMN IF EXISTS enable_folder_view,
                    DROP COLUMN IF EXISTS enable_grouping_movies_into_collections,
                    DROP COLUMN IF EXISTS enable_grouping_shows_into_collections,
                    DROP COLUMN IF EXISTS display_specials_within_seasons,
                    DROP COLUMN IF EXISTS enable_external_content_in_suggestions,
                    DROP COLUMN IF EXISTS cors_hosts,
                    DROP COLUMN IF EXISTS activity_log_retention_days,
                    DROP COLUMN IF EXISTS library_scan_fanout_concurrency,
                    DROP COLUMN IF EXISTS library_metadata_refresh_concurrency;
                "#,
            )
            .await?;
        Ok(())
    }
}
