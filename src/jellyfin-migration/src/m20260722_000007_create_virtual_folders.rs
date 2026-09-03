use sea_orm_migration::prelude::*;

const VIRTUAL_FOLDERS_SQL: &str = r"
    CREATE TABLE IF NOT EXISTS jellyfin.virtual_folders (
        id uuid PRIMARY KEY,
        name text NOT NULL,
        normalized_name text NOT NULL,
        collection_type text,
        library_options jsonb NOT NULL DEFAULT '{}'::jsonb,
        refresh_requested boolean NOT NULL DEFAULT false,
        date_created timestamptz NOT NULL DEFAULT clock_timestamp(),
        date_modified timestamptz NOT NULL DEFAULT clock_timestamp(),
        CONSTRAINT virtual_folders_name_not_blank CHECK (btrim(name) <> ''),
        CONSTRAINT virtual_folders_normalized_name_not_blank CHECK (normalized_name <> ''),
        CONSTRAINT virtual_folders_normalized_name_key UNIQUE (normalized_name),
        CONSTRAINT virtual_folders_options_object CHECK (jsonb_typeof(library_options) = 'object')
    );

    CREATE TABLE IF NOT EXISTS jellyfin.media_paths (
        id uuid PRIMARY KEY,
        virtual_folder_id uuid NOT NULL,
        path text NOT NULL,
        normalized_path text NOT NULL,
        path_ancestors jsonb NOT NULL,
        path_info jsonb NOT NULL DEFAULT '{}'::jsonb,
        date_created timestamptz NOT NULL DEFAULT clock_timestamp(),
        date_modified timestamptz NOT NULL DEFAULT clock_timestamp(),
        CONSTRAINT media_paths_virtual_folder_fkey
            FOREIGN KEY (virtual_folder_id)
            REFERENCES jellyfin.virtual_folders (id)
            ON DELETE CASCADE,
        CONSTRAINT media_paths_path_not_blank CHECK (path <> ''),
        CONSTRAINT media_paths_normalized_path_not_blank CHECK (normalized_path <> ''),
        CONSTRAINT media_paths_normalized_path_key UNIQUE (normalized_path),
        CONSTRAINT media_paths_ancestors_array CHECK (
            jsonb_typeof(path_ancestors) = 'array' AND jsonb_array_length(path_ancestors) > 0
        ),
        CONSTRAINT media_paths_info_object CHECK (jsonb_typeof(path_info) = 'object')
    );

    COMMENT ON COLUMN jellyfin.media_paths.path_ancestors IS
        'Canonical path plus each canonical parent. Writers serialize overlap checks with '
        'pg_advisory_xact_lock(6216744577148670036).';

    CREATE INDEX IF NOT EXISTS virtual_folders_name_trgm_idx
        ON jellyfin.virtual_folders USING gin (name gin_trgm_ops);
    CREATE INDEX IF NOT EXISTS media_paths_folder_path_idx
        ON jellyfin.media_paths (virtual_folder_id, normalized_path);
    CREATE INDEX IF NOT EXISTS media_paths_ancestors_gin_idx
        ON jellyfin.media_paths USING gin (path_ancestors jsonb_ops);

    CREATE OR REPLACE FUNCTION jellyfin.touch_virtual_folder()
    RETURNS trigger LANGUAGE plpgsql AS $function$
    BEGIN
        NEW.date_modified := clock_timestamp();
        RETURN NEW;
    END
    $function$;

    DROP TRIGGER IF EXISTS virtual_folders_touch ON jellyfin.virtual_folders;
    CREATE TRIGGER virtual_folders_touch
        BEFORE UPDATE ON jellyfin.virtual_folders
        FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_virtual_folder();

    DROP TRIGGER IF EXISTS media_paths_touch ON jellyfin.media_paths;
    CREATE TRIGGER media_paths_touch
        BEFORE UPDATE ON jellyfin.media_paths
        FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_virtual_folder();
";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(VIRTUAL_FOLDERS_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                DROP TABLE IF EXISTS jellyfin.media_paths CASCADE;
                DROP TABLE IF EXISTS jellyfin.virtual_folders CASCADE;
                DROP FUNCTION IF EXISTS jellyfin.touch_virtual_folder();
                ",
            )
            .await?;
        Ok(())
    }
}
