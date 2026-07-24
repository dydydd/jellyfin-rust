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
                CREATE INDEX IF NOT EXISTS linked_children_alternate_child_lookup_idx
                    ON jellyfin.linked_children (child_id, parent_id)
                    WHERE child_type IN (2, 3);

                CREATE INDEX IF NOT EXISTS trickplay_infos_manifest_covering_idx
                    ON jellyfin.trickplay_infos (item_id, width)
                    INCLUDE (
                        height, tile_width, tile_height,
                        thumbnail_count, interval, bandwidth
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
                DROP INDEX IF EXISTS jellyfin.trickplay_infos_manifest_covering_idx;
                DROP INDEX IF EXISTS jellyfin.linked_children_alternate_child_lookup_idx;
                ",
            )
            .await?;
        Ok(())
    }
}
