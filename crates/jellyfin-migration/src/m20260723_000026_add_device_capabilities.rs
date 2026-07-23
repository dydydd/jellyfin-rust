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
                ALTER TABLE jellyfin.devices
                    ADD COLUMN IF NOT EXISTS capabilities jsonb NOT NULL DEFAULT '{}'::jsonb;
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_capabilities_object;
                ALTER TABLE jellyfin.devices
                    ADD CONSTRAINT devices_capabilities_object
                        CHECK (jsonb_typeof(capabilities) = 'object');
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
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_capabilities_object;
                ALTER TABLE jellyfin.devices
                    DROP COLUMN IF EXISTS capabilities;
                ",
            )
            .await?;
        Ok(())
    }
}
