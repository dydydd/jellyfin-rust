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
                CREATE INDEX IF NOT EXISTS user_data_version_date_played_idx
                    ON jellyfin.user_data
                        (user_id, item_id, last_played_date DESC NULLS LAST)
                    WHERE last_played_date IS NOT NULL;
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS jellyfin.user_data_version_date_played_idx;")
            .await?;
        Ok(())
    }
}
