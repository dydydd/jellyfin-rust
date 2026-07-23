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
                    ADD COLUMN IF NOT EXISTS additional_users jsonb NOT NULL DEFAULT '[]'::jsonb;
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_additional_users_array;
                ALTER TABLE jellyfin.devices
                    ADD CONSTRAINT devices_additional_users_array
                        CHECK (jsonb_typeof(additional_users) = 'array');
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
                    DROP CONSTRAINT IF EXISTS devices_additional_users_array;
                ALTER TABLE jellyfin.devices
                    DROP COLUMN IF EXISTS additional_users;
                ",
            )
            .await?;
        Ok(())
    }
}
