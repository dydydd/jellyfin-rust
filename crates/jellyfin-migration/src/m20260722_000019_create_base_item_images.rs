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
                CREATE TABLE IF NOT EXISTS jellyfin.base_item_images (
                    item_id uuid NOT NULL,
                    image_type smallint NOT NULL,
                    image_index integer NOT NULL,
                    path text NOT NULL,
                    date_modified timestamptz NOT NULL,
                    width integer,
                    height integer,
                    blurhash text,
                    CONSTRAINT base_item_images_pkey
                        PRIMARY KEY (item_id, image_type, image_index),
                    CONSTRAINT base_item_images_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE,
                    CONSTRAINT base_item_images_type_valid
                        CHECK (image_type BETWEEN 0 AND 12),
                    CONSTRAINT base_item_images_index_nonnegative
                        CHECK (image_index >= 0),
                    CONSTRAINT base_item_images_path_not_blank
                        CHECK (path !~ '^[[:space:]]*$'),
                    CONSTRAINT base_item_images_width_positive
                        CHECK (width IS NULL OR width > 0),
                    CONSTRAINT base_item_images_height_positive
                        CHECK (height IS NULL OR height > 0)
                );

                CREATE INDEX IF NOT EXISTS base_item_images_primary_lookup_idx
                    ON jellyfin.base_item_images (item_id)
                    INCLUDE (path, date_modified, width, height, blurhash)
                    WHERE image_type = 0 AND image_index = 0;

                COMMENT ON TABLE jellyfin.base_item_images IS
                    'Normalized base-item image metadata. The partial covering index '
                    'keeps the DTO primary-image lookup index-only on PostgreSQL.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.base_item_images CASCADE;")
            .await?;
        Ok(())
    }
}
