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
                CREATE TABLE IF NOT EXISTS jellyfin.trickplay_infos (
                    item_id uuid NOT NULL,
                    width integer NOT NULL,
                    height integer NOT NULL,
                    tile_width integer NOT NULL,
                    tile_height integer NOT NULL,
                    thumbnail_count integer NOT NULL,
                    interval integer NOT NULL,
                    bandwidth integer NOT NULL,
                    CONSTRAINT trickplay_infos_pkey PRIMARY KEY (item_id, width),
                    CONSTRAINT trickplay_infos_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE,
                    CONSTRAINT trickplay_infos_width_positive CHECK (width > 0),
                    CONSTRAINT trickplay_infos_height_positive CHECK (height > 0),
                    CONSTRAINT trickplay_infos_tile_width_positive CHECK (tile_width > 0),
                    CONSTRAINT trickplay_infos_tile_height_positive CHECK (tile_height > 0),
                    CONSTRAINT trickplay_infos_thumbnail_count_nonnegative
                        CHECK (thumbnail_count >= 0),
                    CONSTRAINT trickplay_infos_interval_positive CHECK (interval > 0),
                    CONSTRAINT trickplay_infos_bandwidth_nonnegative CHECK (bandwidth >= 0)
                );

                COMMENT ON TABLE jellyfin.trickplay_infos IS
                    'Trickplay tile metadata keyed by item and thumbnail width. '
                    'The composite primary key is also the item-scoped resolution index.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.trickplay_infos CASCADE;")
            .await?;
        Ok(())
    }
}
