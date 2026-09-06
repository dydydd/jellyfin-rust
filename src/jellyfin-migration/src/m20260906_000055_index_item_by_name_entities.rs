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
                CREATE INDEX IF NOT EXISTS base_items_item_by_name_type_clean_name_idx
                    ON jellyfin.base_items (item_type, clean_name, id)
                    WHERE item_type IN (
                        'Genre',
                        'MediaBrowser.Controller.Entities.Genre',
                        'MusicGenre',
                        'MediaBrowser.Controller.Entities.Audio.MusicGenre'
                    )
                      AND clean_name IS NOT NULL;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP INDEX IF EXISTS jellyfin.base_items_item_by_name_type_clean_name_idx;",
            )
            .await?;
        Ok(())
    }
}
