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
                ALTER TABLE jellyfin.chapters
                    ADD COLUMN IF NOT EXISTS image_path text,
                    ADD COLUMN IF NOT EXISTS image_date_modified timestamptz;
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
                ALTER TABLE jellyfin.chapters
                    DROP COLUMN IF EXISTS image_path,
                    DROP COLUMN IF EXISTS image_date_modified;
                ",
            )
            .await?;
        Ok(())
    }
}
