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
                CREATE TABLE IF NOT EXISTS jellyfin.media_segments (
                    id uuid PRIMARY KEY,
                    item_id uuid NOT NULL,
                    segment_type integer NOT NULL,
                    start_ticks bigint NOT NULL,
                    end_ticks bigint NOT NULL,
                    segment_provider_id varchar(64) NOT NULL,
                    CONSTRAINT media_segments_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE,
                    CONSTRAINT media_segments_type_range
                        CHECK (segment_type BETWEEN 0 AND 5),
                    CONSTRAINT media_segments_start_nonnegative
                        CHECK (start_ticks >= 0),
                    CONSTRAINT media_segments_end_after_start
                        CHECK (end_ticks >= start_ticks),
                    CONSTRAINT media_segments_provider_not_empty
                        CHECK (segment_provider_id <> '')
                );
                CREATE INDEX IF NOT EXISTS media_segments_item_type_idx
                    ON jellyfin.media_segments (item_id, segment_type, start_ticks);
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.media_segments CASCADE;")
            .await?;
        Ok(())
    }
}
