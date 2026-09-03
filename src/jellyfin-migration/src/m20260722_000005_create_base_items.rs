use sea_orm_migration::prelude::*;

const TABLES_AND_INDEXES_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS jellyfin.base_items (
        id uuid PRIMARY KEY,
        item_type varchar(256) NOT NULL,
        data jsonb,
        path text,
        parent_id uuid,
        top_parent_id uuid,
        name text,
        sort_name text,
        media_type varchar(64),
        overview text,
        official_rating text,
        index_number integer,
        parent_index_number integer,
        production_year integer,
        runtime_ticks bigint,
        is_folder boolean NOT NULL DEFAULT false,
        is_virtual_item boolean NOT NULL DEFAULT false,
        presentation_unique_key text,
        series_id uuid,
        season_id uuid,
        series_presentation_unique_key text,
        date_created timestamptz NOT NULL DEFAULT clock_timestamp(),
        date_modified timestamptz NOT NULL DEFAULT clock_timestamp(),
        row_version bigint NOT NULL DEFAULT 1,
        CONSTRAINT base_items_parent_id_fkey
            FOREIGN KEY (parent_id) REFERENCES jellyfin.base_items (id)
            ON DELETE CASCADE,
        CONSTRAINT base_items_type_not_empty CHECK (item_type <> ''),
        CONSTRAINT base_items_parent_not_self
            CHECK (parent_id IS NULL OR parent_id <> id),
        CONSTRAINT base_items_runtime_nonnegative
            CHECK (runtime_ticks IS NULL OR runtime_ticks >= 0),
        CONSTRAINT base_items_row_version_positive CHECK (row_version > 0)
    );

    CREATE TABLE IF NOT EXISTS jellyfin.ancestor_ids (
        item_id uuid NOT NULL,
        parent_item_id uuid NOT NULL,
        depth integer NOT NULL,
        PRIMARY KEY (item_id, parent_item_id),
        CONSTRAINT ancestor_ids_item_id_fkey
            FOREIGN KEY (item_id) REFERENCES jellyfin.base_items (id)
            ON DELETE CASCADE,
        CONSTRAINT ancestor_ids_parent_item_id_fkey
            FOREIGN KEY (parent_item_id) REFERENCES jellyfin.base_items (id)
            ON DELETE CASCADE,
        CONSTRAINT ancestor_ids_depth_positive CHECK (depth > 0),
        CONSTRAINT ancestor_ids_one_parent_per_depth UNIQUE (item_id, depth)
    );

    COMMENT ON TABLE jellyfin.ancestor_ids IS
        'Read-optimized closure table matching Jellyfin AncestorId queries. '
        'It avoids ltree extension privileges and recursive CTEs on every item read; '
        'subtree moves rebuild affected rows transactionally. Hierarchy writers must acquire '
        'pg_advisory_xact_lock(4774188637037544781) in a separate statement before validation.';

    CREATE INDEX IF NOT EXISTS base_items_parent_sort_idx
        ON jellyfin.base_items (parent_id, sort_name, id)
        WHERE parent_id IS NOT NULL;
    CREATE INDEX IF NOT EXISTS base_items_path_hash_idx
        ON jellyfin.base_items USING hash (path)
        WHERE path IS NOT NULL;
    CREATE INDEX IF NOT EXISTS base_items_top_type_sort_idx
        ON jellyfin.base_items
            (top_parent_id, item_type, is_virtual_item, sort_name, id);
    CREATE INDEX IF NOT EXISTS ancestor_ids_item_depth_idx
        ON jellyfin.ancestor_ids (item_id, depth, parent_item_id);
    CREATE INDEX IF NOT EXISTS ancestor_ids_parent_depth_idx
        ON jellyfin.ancestor_ids (parent_item_id, depth, item_id);
";

const FUNCTIONS_AND_TRIGGERS_SQL: &str = r"
    CREATE OR REPLACE FUNCTION jellyfin.base_items_validate_hierarchy()
    RETURNS trigger LANGUAGE plpgsql AS $function$
    DECLARE
        resolved_top_parent uuid;
    BEGIN
        IF TG_OP = 'UPDATE'
           AND NEW.parent_id IS NOT DISTINCT FROM OLD.parent_id THEN
            NEW.top_parent_id := OLD.top_parent_id;
            RETURN NEW;
        END IF;

        IF NEW.parent_id IS NULL THEN
            NEW.top_parent_id := NULL;
            RETURN NEW;
        END IF;

        IF NEW.parent_id = NEW.id
           OR (TG_OP = 'UPDATE' AND EXISTS (
                SELECT 1
                FROM jellyfin.ancestor_ids
                WHERE item_id = NEW.parent_id
                  AND parent_item_id = NEW.id
           )) THEN
            RAISE EXCEPTION 'base item hierarchy cannot contain a cycle'
                USING ERRCODE = '23514',
                      CONSTRAINT = 'base_items_hierarchy_acyclic';
        END IF;

        SELECT COALESCE(top_parent_id, id)
        INTO resolved_top_parent
        FROM jellyfin.base_items
        WHERE id = NEW.parent_id;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'base item parent % does not exist', NEW.parent_id
                USING ERRCODE = '23503',
                      CONSTRAINT = 'base_items_parent_id_fkey';
        END IF;

        NEW.top_parent_id := resolved_top_parent;
        RETURN NEW;
    END
    $function$;

    CREATE OR REPLACE FUNCTION jellyfin.base_items_rebuild_hierarchy()
    RETURNS trigger LANGUAGE plpgsql AS $function$
    BEGIN
        IF TG_OP = 'INSERT' THEN
            INSERT INTO jellyfin.ancestor_ids (item_id, parent_item_id, depth)
            SELECT NEW.id, NEW.parent_id, 1
            WHERE NEW.parent_id IS NOT NULL
            UNION ALL
            SELECT NEW.id, parent_item_id, depth + 1
            FROM jellyfin.ancestor_ids
            WHERE item_id = NEW.parent_id;
            RETURN NEW;
        END IF;

        IF NEW.parent_id IS NOT DISTINCT FROM OLD.parent_id THEN
            RETURN NEW;
        END IF;

        WITH RECURSIVE subtree(id) AS (
            SELECT NEW.id
            UNION ALL
            SELECT child.id
            FROM jellyfin.base_items child
            JOIN subtree parent ON child.parent_id = parent.id
        )
        DELETE FROM jellyfin.ancestor_ids closure
        USING subtree
        WHERE closure.item_id = subtree.id;

        WITH RECURSIVE
        subtree(id) AS (
            SELECT NEW.id
            UNION ALL
            SELECT child.id
            FROM jellyfin.base_items child
            JOIN subtree parent ON child.parent_id = parent.id
        ),
        paths(item_id, parent_item_id, depth) AS (
            SELECT item.id, item.parent_id, 1
            FROM jellyfin.base_items item
            JOIN subtree ON subtree.id = item.id
            WHERE item.parent_id IS NOT NULL
            UNION ALL
            SELECT paths.item_id, parent.parent_id, paths.depth + 1
            FROM paths
            JOIN jellyfin.base_items parent ON parent.id = paths.parent_item_id
            WHERE parent.parent_id IS NOT NULL
        )
        INSERT INTO jellyfin.ancestor_ids (item_id, parent_item_id, depth)
        SELECT item_id, parent_item_id, depth
        FROM paths;

        WITH RECURSIVE descendants(id) AS (
            SELECT child.id
            FROM jellyfin.base_items child
            WHERE child.parent_id = NEW.id
            UNION ALL
            SELECT child.id
            FROM jellyfin.base_items child
            JOIN descendants parent ON child.parent_id = parent.id
        )
        UPDATE jellyfin.base_items item
        SET top_parent_id = COALESCE(NEW.top_parent_id, NEW.id)
        FROM descendants
        WHERE item.id = descendants.id
          AND item.top_parent_id IS DISTINCT FROM
              COALESCE(NEW.top_parent_id, NEW.id);

        RETURN NEW;
    END
    $function$;

    CREATE OR REPLACE FUNCTION jellyfin.touch_base_item_row_version()
    RETURNS trigger LANGUAGE plpgsql AS $function$
    BEGIN
        NEW.row_version := OLD.row_version + 1;
        NEW.date_modified := clock_timestamp();
        RETURN NEW;
    END
    $function$;

    DROP TRIGGER IF EXISTS base_items_validate_insert ON jellyfin.base_items;
    CREATE TRIGGER base_items_validate_insert
        BEFORE INSERT ON jellyfin.base_items
        FOR EACH ROW EXECUTE FUNCTION jellyfin.base_items_validate_hierarchy();
    DROP TRIGGER IF EXISTS base_items_validate_parent_update ON jellyfin.base_items;
    CREATE TRIGGER base_items_validate_parent_update
        BEFORE UPDATE OF parent_id ON jellyfin.base_items
        FOR EACH ROW EXECUTE FUNCTION jellyfin.base_items_validate_hierarchy();
    DROP TRIGGER IF EXISTS base_items_rebuild_insert ON jellyfin.base_items;
    CREATE TRIGGER base_items_rebuild_insert
        AFTER INSERT ON jellyfin.base_items
        FOR EACH ROW EXECUTE FUNCTION jellyfin.base_items_rebuild_hierarchy();
    DROP TRIGGER IF EXISTS base_items_rebuild_parent_update ON jellyfin.base_items;
    CREATE TRIGGER base_items_rebuild_parent_update
        AFTER UPDATE OF parent_id ON jellyfin.base_items
        FOR EACH ROW EXECUTE FUNCTION jellyfin.base_items_rebuild_hierarchy();
    DROP TRIGGER IF EXISTS base_items_touch_row_version ON jellyfin.base_items;
    CREATE TRIGGER base_items_touch_row_version
        BEFORE UPDATE ON jellyfin.base_items
        FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_base_item_row_version();

    INSERT INTO jellyfin.base_items (id, item_type, name)
    VALUES (
        '00000000-0000-0000-0000-000000000001',
        'PLACEHOLDER',
        'This is a placeholder item for UserData that has been detached from its original item'
    )
    ON CONFLICT (id) DO NOTHING;
";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(TABLES_AND_INDEXES_SQL)
            .await?;
        connection
            .execute_unprepared(FUNCTIONS_AND_TRIGGERS_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                DROP TABLE IF EXISTS jellyfin.ancestor_ids CASCADE;
                DROP TABLE IF EXISTS jellyfin.base_items CASCADE;
                DROP FUNCTION IF EXISTS jellyfin.base_items_validate_hierarchy();
                DROP FUNCTION IF EXISTS jellyfin.base_items_rebuild_hierarchy();
                DROP FUNCTION IF EXISTS jellyfin.touch_base_item_row_version();
                ",
            )
            .await?;
        Ok(())
    }
}
