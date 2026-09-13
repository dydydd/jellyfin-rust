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
                ALTER TABLE jellyfin.user_data
                    ADD COLUMN is_hidden_from_resume boolean NOT NULL DEFAULT false;

                DROP INDEX IF EXISTS jellyfin.user_data_resume_idx;
                DROP INDEX IF EXISTS jellyfin.user_data_resume_order_idx;
                CREATE INDEX user_data_resume_idx
                    ON jellyfin.user_data (user_id, item_id)
                    WHERE playback_position_ticks > 0
                      AND is_hidden_from_resume = false;
                CREATE INDEX user_data_resume_order_idx
                    ON jellyfin.user_data
                        (user_id, item_id, last_played_date DESC NULLS LAST, custom_data_key)
                    INCLUDE (playback_position_ticks)
                    WHERE playback_position_ticks > 0
                      AND is_hidden_from_resume = false;
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
                DROP INDEX IF EXISTS jellyfin.user_data_resume_idx;
                DROP INDEX IF EXISTS jellyfin.user_data_resume_order_idx;
                CREATE INDEX user_data_resume_idx
                    ON jellyfin.user_data (user_id, item_id)
                    WHERE playback_position_ticks > 0;
                CREATE INDEX user_data_resume_order_idx
                    ON jellyfin.user_data
                        (user_id, item_id, last_played_date DESC NULLS LAST, custom_data_key)
                    INCLUDE (playback_position_ticks)
                    WHERE playback_position_ticks > 0;

                ALTER TABLE jellyfin.user_data
                    DROP COLUMN is_hidden_from_resume;
                ",
            )
            .await?;
        Ok(())
    }
}
