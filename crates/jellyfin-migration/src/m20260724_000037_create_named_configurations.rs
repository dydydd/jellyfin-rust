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
                CREATE TABLE IF NOT EXISTS jellyfin.named_configurations (
                    key text PRIMARY KEY,
                    configuration jsonb NOT NULL DEFAULT '{}'::jsonb,
                    row_version bigint NOT NULL DEFAULT 1,
                    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    CONSTRAINT named_configurations_key_not_blank
                        CHECK (key !~ '^[[:space:]]*$'),
                    CONSTRAINT named_configurations_object
                        CHECK (jsonb_typeof(configuration) = 'object')
                );

                DROP TRIGGER IF EXISTS named_configurations_touch_row_version
                    ON jellyfin.named_configurations;
                CREATE TRIGGER named_configurations_touch_row_version
                    BEFORE UPDATE ON jellyfin.named_configurations
                    FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_row_version();
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.named_configurations CASCADE;")
            .await?;
        Ok(())
    }
}
