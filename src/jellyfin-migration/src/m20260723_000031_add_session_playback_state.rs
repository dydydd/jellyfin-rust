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
                    ADD COLUMN IF NOT EXISTS play_state jsonb NOT NULL DEFAULT '{}'::jsonb,
                    ADD COLUMN IF NOT EXISTS now_playing_item jsonb,
                    ADD COLUMN IF NOT EXISTS now_playing_queue jsonb NOT NULL DEFAULT '[]'::jsonb,
                    ADD COLUMN IF NOT EXISTS playlist_item_id varchar(256),
                    ADD COLUMN IF NOT EXISTS date_last_paused timestamptz;
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_play_state_object;
                ALTER TABLE jellyfin.devices
                    ADD CONSTRAINT devices_play_state_object
                        CHECK (jsonb_typeof(play_state) = 'object');
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_now_playing_item_object;
                ALTER TABLE jellyfin.devices
                    ADD CONSTRAINT devices_now_playing_item_object CHECK (
                        now_playing_item IS NULL
                        OR jsonb_typeof(now_playing_item) = 'object'
                    );
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_now_playing_queue_array;
                ALTER TABLE jellyfin.devices
                    ADD CONSTRAINT devices_now_playing_queue_array
                        CHECK (jsonb_typeof(now_playing_queue) = 'array');
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
                    DROP CONSTRAINT IF EXISTS devices_now_playing_queue_array;
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_now_playing_item_object;
                ALTER TABLE jellyfin.devices
                    DROP CONSTRAINT IF EXISTS devices_play_state_object;
                ALTER TABLE jellyfin.devices
                    DROP COLUMN IF EXISTS date_last_paused,
                    DROP COLUMN IF EXISTS playlist_item_id,
                    DROP COLUMN IF EXISTS now_playing_queue,
                    DROP COLUMN IF EXISTS now_playing_item,
                    DROP COLUMN IF EXISTS play_state;
                ",
            )
            .await?;
        Ok(())
    }
}
