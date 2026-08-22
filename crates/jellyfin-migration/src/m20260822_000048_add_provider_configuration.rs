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
                    ADD COLUMN IF NOT EXISTS quick_connect_available boolean NOT NULL DEFAULT true;
                ALTER TABLE jellyfin.server_configuration
                    ADD COLUMN IF NOT EXISTS omdb_api_key text NOT NULL DEFAULT '2c9d9507';
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
                    DROP COLUMN IF EXISTS omdb_api_key;
                ALTER TABLE jellyfin.server_configuration
                    DROP COLUMN IF EXISTS quick_connect_available;
                ",
            )
            .await?;
        Ok(())
    }
}
