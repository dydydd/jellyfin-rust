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
                ALTER TABLE jellyfin.base_items
                    ADD COLUMN IF NOT EXISTS primary_version_id uuid;

                DO $block$
                BEGIN
                    IF NOT EXISTS (
                        SELECT 1
                        FROM pg_constraint
                        WHERE conrelid = 'jellyfin.base_items'::regclass
                          AND conname = 'base_items_primary_version_not_self'
                    ) THEN
                        ALTER TABLE jellyfin.base_items
                            ADD CONSTRAINT base_items_primary_version_not_self
                            CHECK (primary_version_id IS NULL OR primary_version_id <> id);
                    END IF;

                    IF NOT EXISTS (
                        SELECT 1
                        FROM pg_constraint
                        WHERE conrelid = 'jellyfin.base_items'::regclass
                          AND conname = 'base_items_primary_version_id_fkey'
                    ) THEN
                        ALTER TABLE jellyfin.base_items
                            ADD CONSTRAINT base_items_primary_version_id_fkey
                            FOREIGN KEY (primary_version_id)
                            REFERENCES jellyfin.base_items (id)
                            ON DELETE SET NULL;
                    END IF;
                END
                $block$;

                CREATE INDEX IF NOT EXISTS base_items_primary_version_id_idx
                    ON jellyfin.base_items (primary_version_id)
                    WHERE primary_version_id IS NOT NULL;
                CREATE INDEX IF NOT EXISTS base_items_version_group_idx
                    ON jellyfin.base_items (
                        presentation_unique_key,
                        ((primary_version_id IS NULL)) DESC,
                        id
                    )
                    INCLUDE (sort_name)
                    WHERE presentation_unique_key IS NOT NULL;
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
                DROP INDEX IF EXISTS jellyfin.base_items_version_group_idx;
                DROP INDEX IF EXISTS jellyfin.base_items_primary_version_id_idx;
                ALTER TABLE jellyfin.base_items
                    DROP CONSTRAINT IF EXISTS base_items_primary_version_id_fkey,
                    DROP CONSTRAINT IF EXISTS base_items_primary_version_not_self,
                    DROP COLUMN IF EXISTS primary_version_id;
                ",
            )
            .await?;
        Ok(())
    }
}
