use sea_orm_migration::prelude::*;

const ITEM_VALUES_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS jellyfin.item_values (
        item_value_id uuid PRIMARY KEY,
        type smallint NOT NULL,
        value text NOT NULL,
        clean_value text NOT NULL,
        CONSTRAINT item_values_type_valid CHECK (type IN (0, 1, 2, 3, 4, 6)),
        CONSTRAINT item_values_value_not_empty CHECK (btrim(value) <> ''),
        CONSTRAINT item_values_clean_value_not_empty CHECK (clean_value <> '')
    );

    CREATE TABLE IF NOT EXISTS jellyfin.item_value_map (
        item_value_id uuid NOT NULL,
        item_id uuid NOT NULL,
        PRIMARY KEY (item_value_id, item_id),
        CONSTRAINT item_value_map_value_fkey
            FOREIGN KEY (item_value_id)
            REFERENCES jellyfin.item_values (item_value_id)
            ON DELETE CASCADE,
        CONSTRAINT item_value_map_item_fkey
            FOREIGN KEY (item_id)
            REFERENCES jellyfin.base_items (id)
            ON DELETE CASCADE
    );

    CREATE UNIQUE INDEX IF NOT EXISTS item_values_type_value_key
        ON jellyfin.item_values (type, value)
        INCLUDE (item_value_id, clean_value);
    CREATE UNIQUE INDEX IF NOT EXISTS item_values_type_clean_value_key
        ON jellyfin.item_values (type, clean_value)
        INCLUDE (item_value_id, value);
    CREATE INDEX IF NOT EXISTS item_value_map_item_idx
        ON jellyfin.item_value_map (item_id, item_value_id);
";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(ITEM_VALUES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS jellyfin.item_value_map CASCADE; \
                 DROP TABLE IF EXISTS jellyfin.item_values CASCADE;",
            )
            .await?;
        Ok(())
    }
}
