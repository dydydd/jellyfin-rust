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
                CREATE TABLE IF NOT EXISTS jellyfin.media_attachments (
                    item_id uuid NOT NULL,
                    attachment_index integer NOT NULL,
                    codec text,
                    codec_tag text,
                    comment text,
                    file_name text,
                    mime_type text,
                    delivery_url text,
                    CONSTRAINT media_attachments_pkey
                        PRIMARY KEY (item_id, attachment_index),
                    CONSTRAINT media_attachments_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE
                );

                COMMENT ON TABLE jellyfin.media_attachments IS
                    'Normalized media-attachment probe data. The item/index '
                    'primary key supports PostgreSQL ordered item-scoped reads '
                    'without additional write-heavy secondary indexes.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.media_attachments CASCADE;")
            .await?;
        Ok(())
    }
}
