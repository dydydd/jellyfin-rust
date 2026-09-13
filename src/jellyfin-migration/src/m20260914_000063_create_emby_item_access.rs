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
                CREATE TABLE IF NOT EXISTS jellyfin.emby_item_access (
                    user_id uuid NOT NULL,
                    item_id uuid NOT NULL,
                    access_level smallint NOT NULL,
                    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    PRIMARY KEY (user_id, item_id),
                    CONSTRAINT emby_item_access_user_id_fkey
                        FOREIGN KEY (user_id) REFERENCES jellyfin.users (id) ON DELETE CASCADE,
                    CONSTRAINT emby_item_access_item_id_fkey
                        FOREIGN KEY (item_id) REFERENCES jellyfin.base_items (id) ON DELETE CASCADE,
                    CONSTRAINT emby_item_access_level_valid
                        CHECK (access_level BETWEEN 1 AND 4)
                );

                CREATE INDEX IF NOT EXISTS emby_item_access_item_user_idx
                    ON jellyfin.emby_item_access (item_id, user_id);

                COMMENT ON TABLE jellyfin.emby_item_access IS
                    'Private Emby UserItemShareLevel assignments; not part of the Jellyfin API contract.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.emby_item_access CASCADE;")
            .await?;
        Ok(())
    }
}
