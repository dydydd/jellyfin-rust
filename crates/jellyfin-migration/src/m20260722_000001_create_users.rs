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
                CREATE SCHEMA IF NOT EXISTS jellyfin;
                CREATE EXTENSION IF NOT EXISTS pg_trgm;

                CREATE TABLE IF NOT EXISTS jellyfin.users (
                    id uuid PRIMARY KEY,
                    username varchar(255) NOT NULL,
                    normalized_username varchar(255) NOT NULL,
                    password_hash text,
                    must_update_password boolean NOT NULL DEFAULT false,
                    is_administrator boolean NOT NULL DEFAULT false,
                    is_hidden boolean NOT NULL DEFAULT true,
                    is_disabled boolean NOT NULL DEFAULT false,
                    enable_auto_login boolean NOT NULL DEFAULT false,
                    last_login_date timestamptz,
                    last_activity_date timestamptz,
                    policy jsonb NOT NULL DEFAULT '{}'::jsonb,
                    preferences jsonb NOT NULL DEFAULT '{}'::jsonb,
                    row_version bigint NOT NULL DEFAULT 1,
                    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
                    CONSTRAINT users_username_not_blank CHECK (btrim(username) <> ''),
                    CONSTRAINT users_normalized_username_key UNIQUE (normalized_username)
                );

                CREATE INDEX IF NOT EXISTS users_active_idx
                    ON jellyfin.users (normalized_username)
                    WHERE NOT is_disabled;
                CREATE INDEX IF NOT EXISTS users_username_trgm_idx
                    ON jellyfin.users USING gin (username gin_trgm_ops);
                CREATE INDEX IF NOT EXISTS users_policy_gin_idx
                    ON jellyfin.users USING gin (policy jsonb_path_ops);

                CREATE OR REPLACE FUNCTION jellyfin.touch_row_version()
                RETURNS trigger LANGUAGE plpgsql AS $function$
                BEGIN
                    NEW.updated_at := clock_timestamp();
                    NEW.row_version := OLD.row_version + 1;
                    RETURN NEW;
                END
                $function$;

                DROP TRIGGER IF EXISTS users_touch_row_version ON jellyfin.users;
                CREATE TRIGGER users_touch_row_version
                    BEFORE UPDATE ON jellyfin.users
                    FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_row_version();
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
                DROP TABLE IF EXISTS jellyfin.users CASCADE;
                DROP FUNCTION IF EXISTS jellyfin.touch_row_version();
                DROP SCHEMA IF EXISTS jellyfin;
                ",
            )
            .await?;
        Ok(())
    }
}
