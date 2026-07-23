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
                    ADD COLUMN IF NOT EXISTS min_resume_pct integer NOT NULL DEFAULT 5,
                    ADD COLUMN IF NOT EXISTS max_resume_pct integer NOT NULL DEFAULT 90,
                    ADD COLUMN IF NOT EXISTS min_resume_duration_seconds integer NOT NULL DEFAULT 300,
                    ADD COLUMN IF NOT EXISTS min_audiobook_resume integer NOT NULL DEFAULT 5,
                    ADD COLUMN IF NOT EXISTS max_audiobook_resume integer NOT NULL DEFAULT 5;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                ALTER TABLE jellyfin.server_configuration
                    DROP COLUMN IF EXISTS max_audiobook_resume,
                    DROP COLUMN IF EXISTS min_audiobook_resume,
                    DROP COLUMN IF EXISTS min_resume_duration_seconds,
                    DROP COLUMN IF EXISTS max_resume_pct,
                    DROP COLUMN IF EXISTS min_resume_pct;
                ",
            )
            .await?;
        Ok(())
    }
}
