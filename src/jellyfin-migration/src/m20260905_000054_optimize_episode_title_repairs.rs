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
                CREATE INDEX IF NOT EXISTS base_items_primary_episode_series_idx
                    ON jellyfin.base_items (series_id, id)
                    WHERE item_type = 'Episode'
                      AND primary_version_id IS NULL;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS jellyfin.base_items_primary_episode_series_idx;",
            )
            .await?;
        Ok(())
    }
}
