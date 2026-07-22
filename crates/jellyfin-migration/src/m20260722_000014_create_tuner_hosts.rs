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
                CREATE TABLE IF NOT EXISTS jellyfin.tuner_hosts (
                    id uuid PRIMARY KEY,
                    url text NOT NULL,
                    tuner_type text NOT NULL,
                    device_id text,
                    friendly_name text,
                    import_favorites_only boolean NOT NULL DEFAULT false,
                    allow_hw_transcoding boolean NOT NULL DEFAULT true,
                    allow_fmp4_transcoding_container boolean NOT NULL DEFAULT false,
                    allow_stream_sharing boolean NOT NULL DEFAULT true,
                    fallback_max_streaming_bitrate integer NOT NULL DEFAULT 30000000,
                    enable_stream_looping boolean NOT NULL DEFAULT false,
                    source text,
                    tuner_count integer NOT NULL DEFAULT 0,
                    user_agent text,
                    ignore_dts boolean NOT NULL DEFAULT true,
                    read_at_native_framerate boolean NOT NULL DEFAULT false,
                    date_created timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    date_modified timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    row_version bigint NOT NULL DEFAULT 1,
                    CONSTRAINT tuner_hosts_url_not_blank CHECK (btrim(url) <> ''),
                    CONSTRAINT tuner_hosts_type_not_blank CHECK (btrim(tuner_type) <> ''),
                    CONSTRAINT tuner_hosts_tuner_count_nonnegative CHECK (tuner_count >= 0),
                    CONSTRAINT tuner_hosts_bitrate_nonnegative
                        CHECK (fallback_max_streaming_bitrate >= 0)
                );

                CREATE OR REPLACE FUNCTION jellyfin.touch_tuner_host()
                RETURNS trigger
                LANGUAGE plpgsql
                AS $function$
                BEGIN
                    NEW.date_created := OLD.date_created;
                    NEW.date_modified := CURRENT_TIMESTAMP;
                    NEW.row_version := OLD.row_version + 1;
                    RETURN NEW;
                END;
                $function$;

                DROP TRIGGER IF EXISTS tuner_hosts_touch ON jellyfin.tuner_hosts;
                CREATE TRIGGER tuner_hosts_touch
                    BEFORE UPDATE ON jellyfin.tuner_hosts
                    FOR EACH ROW EXECUTE FUNCTION jellyfin.touch_tuner_host();
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS jellyfin.tuner_hosts CASCADE; \
                 DROP FUNCTION IF EXISTS jellyfin.touch_tuner_host();",
            )
            .await?;
        Ok(())
    }
}
