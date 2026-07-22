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
                ALTER TABLE jellyfin.server_configuration
                    ADD COLUMN IF NOT EXISTS content_types jsonb NOT NULL DEFAULT '[]'::jsonb;

                DO $$
                BEGIN
                    IF NOT EXISTS (
                        SELECT 1
                        FROM pg_constraint
                        WHERE connamespace = 'jellyfin'::regnamespace
                          AND conrelid = 'jellyfin.server_configuration'::regclass
                          AND conname = 'server_configuration_content_types_array'
                    ) THEN
                        ALTER TABLE jellyfin.server_configuration
                            ADD CONSTRAINT server_configuration_content_types_array
                            CHECK (jsonb_typeof(content_types) = 'array');
                    END IF;
                END
                $$;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.server_configuration \
                 DROP COLUMN IF EXISTS content_types;",
            )
            .await?;
        Ok(())
    }
}
