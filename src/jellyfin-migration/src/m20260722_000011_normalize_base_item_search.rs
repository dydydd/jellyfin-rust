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
                SELECT pg_advisory_xact_lock(6001419748217613131);

                CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;

                CREATE OR REPLACE FUNCTION jellyfin.normalize_search_text(value text)
                RETURNS text
                LANGUAGE sql
                IMMUTABLE
                PARALLEL SAFE
                STRICT
                SET search_path = pg_catalog, public
                AS $function$
                    SELECT trim(regexp_replace(
                        lower(public.unaccent('public.unaccent'::regdictionary, value)),
                        '[^[:alnum:]]+',
                        ' ',
                        'g'
                    ))
                $function$;

                ALTER TABLE jellyfin.base_items
                    ADD COLUMN IF NOT EXISTS clean_name text
                    GENERATED ALWAYS AS (jellyfin.normalize_search_text(name)) STORED;

                DROP INDEX IF EXISTS jellyfin.base_items_name_trgm_idx;
                CREATE INDEX IF NOT EXISTS base_items_clean_name_trgm_idx
                    ON jellyfin.base_items USING gin (clean_name gin_trgm_ops)
                    WHERE clean_name IS NOT NULL;

                COMMENT ON COLUMN jellyfin.base_items.clean_name IS
                    'Stored punctuation- and accent-insensitive search text generated from name.';
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
                DROP INDEX IF EXISTS jellyfin.base_items_clean_name_trgm_idx;
                ALTER TABLE jellyfin.base_items DROP COLUMN IF EXISTS clean_name;
                DROP FUNCTION IF EXISTS jellyfin.normalize_search_text(text);
                CREATE INDEX IF NOT EXISTS base_items_name_trgm_idx
                    ON jellyfin.base_items USING gin (name gin_trgm_ops)
                    WHERE name IS NOT NULL;
                ",
            )
            .await?;
        Ok(())
    }
}
