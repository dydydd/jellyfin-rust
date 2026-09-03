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
                    ADD COLUMN IF NOT EXISTS trickplay_options jsonb NOT NULL DEFAULT
                    '{"EnableHwAcceleration":false,"EnableHwEncoding":false,"EnableKeyFrameOnlyExtraction":false,"ScanBehavior":"NonBlocking","ProcessPriority":"BelowNormal","Interval":10000,"WidthResolutions":[320],"TileWidth":10,"TileHeight":10,"Qscale":4,"JpegQuality":90,"ProcessThreads":1}'::jsonb;

                DO $$
                BEGIN
                    IF NOT EXISTS (
                        SELECT 1 FROM pg_constraint
                        WHERE connamespace = 'jellyfin'::regnamespace
                          AND conrelid = 'jellyfin.server_configuration'::regclass
                          AND conname = 'server_configuration_trickplay_options_object'
                    ) THEN
                        ALTER TABLE jellyfin.server_configuration
                            ADD CONSTRAINT server_configuration_trickplay_options_object
                            CHECK (jsonb_typeof(trickplay_options) = 'object');
                    END IF;
                END
                $$;
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.server_configuration DROP COLUMN IF EXISTS trickplay_options;",
            )
            .await?;
        Ok(())
    }
}
