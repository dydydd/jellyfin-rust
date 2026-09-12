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
                ALTER TABLE jellyfin.device_options
                    ALTER COLUMN id TYPE integer USING id::integer;
                ALTER SEQUENCE IF EXISTS jellyfin.device_options_id_seq AS integer;
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
                ALTER TABLE jellyfin.device_options
                    ALTER COLUMN id TYPE bigint USING id::bigint;
                ALTER SEQUENCE IF EXISTS jellyfin.device_options_id_seq AS bigint;
                ",
            )
            .await?;
        Ok(())
    }
}
