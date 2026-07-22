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
                CREATE TABLE IF NOT EXISTS jellyfin.keyframe_data (
                    item_id uuid PRIMARY KEY,
                    total_duration bigint NOT NULL,
                    keyframe_ticks jsonb NOT NULL DEFAULT '[]'::jsonb,
                    CONSTRAINT keyframe_data_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE,
                    CONSTRAINT keyframe_data_ticks_array
                        CHECK (jsonb_typeof(keyframe_ticks) = 'array')
                );

                COMMENT ON COLUMN jellyfin.keyframe_data.keyframe_ticks IS
                    'Native JSONB preserves the official keyframe tick collection. '
                    'Element decoding remains a repository concern so one semantically '
                    'corrupt row can be identified and skipped by backup consumers.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.keyframe_data CASCADE;")
            .await?;
        Ok(())
    }
}
