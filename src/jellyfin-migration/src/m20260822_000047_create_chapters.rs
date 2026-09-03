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
                CREATE TABLE IF NOT EXISTS jellyfin.chapters (
                    id uuid PRIMARY KEY,
                    item_id uuid NOT NULL,
                    index_number integer NOT NULL,
                    start_position_ticks bigint NOT NULL,
                    end_position_ticks bigint NOT NULL,
                    name varchar(1024),
                    CONSTRAINT chapters_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE,
                    CONSTRAINT chapters_start_nonnegative
                        CHECK (start_position_ticks >= 0),
                    CONSTRAINT chapters_end_after_start
                        CHECK (end_position_ticks >= start_position_ticks),
                    CONSTRAINT chapters_item_index_unique
                        UNIQUE (item_id, index_number)
                );
                CREATE INDEX IF NOT EXISTS chapters_item_created_idx
                    ON jellyfin.chapters (item_id, index_number);
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.chapters CASCADE;")
            .await?;
        Ok(())
    }
}
