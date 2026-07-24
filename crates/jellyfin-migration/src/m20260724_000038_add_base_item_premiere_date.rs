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
                ALTER TABLE jellyfin.base_items
                    ADD COLUMN IF NOT EXISTS premiere_date timestamptz;

                CREATE INDEX IF NOT EXISTS base_items_episode_premiere_date_idx
                    ON jellyfin.base_items (premiere_date, sort_name, id)
                    WHERE item_type = 'Episode' AND premiere_date IS NOT NULL;
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
                DROP INDEX IF EXISTS jellyfin.base_items_episode_premiere_date_idx;
                ALTER TABLE jellyfin.base_items
                    DROP COLUMN IF EXISTS premiere_date;
                ",
            )
            .await?;
        Ok(())
    }
}
