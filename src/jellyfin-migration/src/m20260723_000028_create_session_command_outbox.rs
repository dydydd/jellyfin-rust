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
                CREATE TABLE IF NOT EXISTS jellyfin.session_command_outbox (
                    id uuid PRIMARY KEY,
                    target_session_id varchar(64) NOT NULL,
                    controlling_session_id varchar(64),
                    message_type varchar(64) NOT NULL,
                    payload jsonb NOT NULL,
                    date_created timestamptz NOT NULL DEFAULT clock_timestamp(),
                    CONSTRAINT session_command_target_not_empty CHECK (target_session_id <> ''),
                    CONSTRAINT session_command_message_type_not_empty CHECK (message_type <> ''),
                    CONSTRAINT session_command_payload_object CHECK (jsonb_typeof(payload) = 'object')
                );
                CREATE INDEX IF NOT EXISTS session_command_target_created_idx
                    ON jellyfin.session_command_outbox (target_session_id, date_created, id)
                    INCLUDE (message_type);
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
                DROP TABLE IF EXISTS jellyfin.session_command_outbox CASCADE;
                ",
            )
            .await?;
        Ok(())
    }
}
