use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.server_configuration \
                 ADD COLUMN IF NOT EXISTS enable_case_sensitive_item_ids \
                 boolean NOT NULL DEFAULT true;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.server_configuration \
                 DROP COLUMN IF EXISTS enable_case_sensitive_item_ids;",
            )
            .await?;
        Ok(())
    }
}
