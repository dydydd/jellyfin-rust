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
                CREATE INDEX IF NOT EXISTS devices_lower_device_activity_idx
                    ON jellyfin.devices (lower(device_id), date_last_activity DESC)
                    WHERE is_active;
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
                DROP INDEX IF EXISTS jellyfin.devices_lower_device_activity_idx;
                ",
            )
            .await?;
        Ok(())
    }
}
