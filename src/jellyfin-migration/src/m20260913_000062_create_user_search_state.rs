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
                CREATE TABLE IF NOT EXISTS jellyfin.user_search_state (
                    user_id uuid PRIMARY KEY,
                    was_searched boolean NOT NULL DEFAULT false,
                    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    CONSTRAINT user_search_state_user_id_fkey
                        FOREIGN KEY (user_id) REFERENCES jellyfin.users (id) ON DELETE CASCADE
                );
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.user_search_state CASCADE;")
            .await?;
        Ok(())
    }
}
