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
                    ADD COLUMN IF NOT EXISTS official_rating text;

                CREATE INDEX IF NOT EXISTS base_items_official_rating_idx
                    ON jellyfin.base_items (official_rating, sort_name, id)
                    WHERE official_rating IS NOT NULL AND official_rating <> '';
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
                DROP INDEX IF EXISTS jellyfin.base_items_official_rating_idx;
                ALTER TABLE jellyfin.base_items
                    DROP COLUMN IF EXISTS official_rating;
                ",
            )
            .await?;
        Ok(())
    }
}
