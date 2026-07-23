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
                    ADD COLUMN IF NOT EXISTS now_viewing_item jsonb;
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_now_viewing_item_object;
                ALTER TABLE jellyfin.devices
                    ADD CONSTRAINT devices_now_viewing_item_object
                        CHECK (
                            now_viewing_item IS NULL
                            OR jsonb_typeof(now_viewing_item) = 'object'
                        );
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
                    DROP CONSTRAINT IF EXISTS devices_now_viewing_item_object;
                ALTER TABLE jellyfin.devices
                    DROP COLUMN IF EXISTS now_viewing_item;
                ",
            )
            .await?;
        Ok(())
    }
}
