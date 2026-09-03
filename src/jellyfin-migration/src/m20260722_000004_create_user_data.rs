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
                CREATE TABLE IF NOT EXISTS jellyfin.user_data (
                    item_id uuid NOT NULL,
                    user_id uuid NOT NULL,
                    custom_data_key text NOT NULL,
                    rating double precision,
                    playback_position_ticks bigint NOT NULL DEFAULT 0,
                    play_count integer NOT NULL DEFAULT 0,
                    is_favorite boolean NOT NULL DEFAULT false,
                    last_played_date timestamptz,
                    played boolean NOT NULL DEFAULT false,
                    audio_stream_index integer,
                    subtitle_stream_index integer,
                    likes boolean,
                    retention_date timestamptz,
                    PRIMARY KEY (item_id, user_id, custom_data_key),
                    CONSTRAINT user_data_user_id_fkey
                        FOREIGN KEY (user_id) REFERENCES jellyfin.users (id) ON DELETE CASCADE,
                    CONSTRAINT user_data_rating_range
                        CHECK (rating IS NULL OR rating BETWEEN 0 AND 10),
                    CONSTRAINT user_data_position_nonnegative CHECK (playback_position_ticks >= 0),
                    CONSTRAINT user_data_play_count_nonnegative CHECK (play_count >= 0)
                );

                CREATE INDEX IF NOT EXISTS user_data_played_idx
                    ON jellyfin.user_data (user_id, item_id)
                    WHERE played;
                CREATE INDEX IF NOT EXISTS user_data_favorite_idx
                    ON jellyfin.user_data (user_id, item_id)
                    WHERE is_favorite;
                CREATE INDEX IF NOT EXISTS user_data_resume_idx
                    ON jellyfin.user_data (user_id, item_id)
                    WHERE playback_position_ticks > 0;
                CREATE INDEX IF NOT EXISTS user_data_last_played_idx
                    ON jellyfin.user_data (user_id, last_played_date DESC, item_id)
                    WHERE last_played_date IS NOT NULL;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.user_data CASCADE;")
            .await?;
        Ok(())
    }
}
