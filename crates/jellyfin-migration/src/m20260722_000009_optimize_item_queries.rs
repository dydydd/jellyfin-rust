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
                CREATE INDEX IF NOT EXISTS base_items_name_trgm_idx
                    ON jellyfin.base_items USING gin (name gin_trgm_ops)
                    WHERE name IS NOT NULL;

                CREATE INDEX IF NOT EXISTS user_data_resume_order_idx
                    ON jellyfin.user_data
                        (user_id, item_id, last_played_date DESC NULLS LAST, custom_data_key)
                    INCLUDE (playback_position_ticks)
                    WHERE playback_position_ticks > 0;
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
                DROP INDEX IF EXISTS jellyfin.user_data_resume_order_idx;
                DROP INDEX IF EXISTS jellyfin.base_items_name_trgm_idx;
                ",
            )
            .await?;
        Ok(())
    }
}
