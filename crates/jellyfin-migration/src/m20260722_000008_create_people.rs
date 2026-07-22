use sea_orm_migration::prelude::*;

const PEOPLE_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS jellyfin.people (
        id uuid PRIMARY KEY,
        name text NOT NULL,
        clean_name text NOT NULL,
        provider_ids jsonb NOT NULL DEFAULT '{}'::jsonb,
        date_created timestamptz NOT NULL DEFAULT clock_timestamp(),
        date_modified timestamptz NOT NULL DEFAULT clock_timestamp(),
        row_version bigint NOT NULL DEFAULT 1,
        CONSTRAINT people_name_not_blank CHECK (btrim(name) <> ''),
        CONSTRAINT people_clean_name_not_blank CHECK (clean_name <> ''),
        CONSTRAINT people_provider_ids_object CHECK (jsonb_typeof(provider_ids) = 'object'),
        CONSTRAINT people_row_version_positive CHECK (row_version > 0)
    );

    CREATE UNIQUE INDEX IF NOT EXISTS people_clean_name_key
        ON jellyfin.people (clean_name)
        INCLUDE (id, name, provider_ids, row_version, date_created, date_modified);
    CREATE INDEX IF NOT EXISTS people_name_exact_idx
        ON jellyfin.people (name)
        INCLUDE (id, clean_name, provider_ids, row_version, date_created, date_modified);
    CREATE INDEX IF NOT EXISTS people_provider_ids_gin_idx
        ON jellyfin.people USING gin (provider_ids jsonb_path_ops);

    CREATE TABLE IF NOT EXISTS jellyfin.people_base_item_map (
        item_id uuid NOT NULL,
        person_id uuid NOT NULL,
        person_type text NOT NULL,
        role text NOT NULL DEFAULT '',
        sort_order integer,
        list_order integer NOT NULL,
        PRIMARY KEY (item_id, person_id, person_type, role),
        CONSTRAINT people_map_item_fkey
            FOREIGN KEY (item_id) REFERENCES jellyfin.base_items (id)
            ON DELETE CASCADE,
        CONSTRAINT people_map_person_fkey
            FOREIGN KEY (person_id) REFERENCES jellyfin.people (id)
            ON DELETE CASCADE,
        CONSTRAINT people_map_type_not_blank CHECK (btrim(person_type) <> ''),
        CONSTRAINT people_map_list_order_nonnegative CHECK (list_order >= 0),
        CONSTRAINT people_map_sort_order_nonnegative CHECK (sort_order IS NULL OR sort_order >= 0)
    );

    CREATE INDEX IF NOT EXISTS people_map_item_order_idx
        ON jellyfin.people_base_item_map (item_id, list_order, person_id)
        INCLUDE (person_type, role, sort_order);
    CREATE INDEX IF NOT EXISTS people_map_person_item_idx
        ON jellyfin.people_base_item_map (person_id, item_id)
        INCLUDE (person_type, role, sort_order, list_order);

    CREATE OR REPLACE FUNCTION jellyfin.touch_person_row_version()
    RETURNS trigger LANGUAGE plpgsql AS $function$
    BEGIN
        NEW.date_modified := clock_timestamp();
        NEW.row_version := OLD.row_version + 1;
        RETURN NEW;
    END
    $function$;

    DROP TRIGGER IF EXISTS people_touch_row_version ON jellyfin.people;
    CREATE TRIGGER people_touch_row_version
        BEFORE UPDATE ON jellyfin.people
        FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_person_row_version();
";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(PEOPLE_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                DROP TABLE IF EXISTS jellyfin.people_base_item_map CASCADE;
                DROP TABLE IF EXISTS jellyfin.people CASCADE;
                DROP FUNCTION IF EXISTS jellyfin.touch_person_row_version();
                ",
            )
            .await?;
        Ok(())
    }
}
