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
                CREATE TABLE IF NOT EXISTS jellyfin.playlists (
                    playlist_id uuid PRIMARY KEY
                        REFERENCES jellyfin.base_items (id) ON DELETE CASCADE,
                    owner_user_id uuid
                        REFERENCES jellyfin.users (id) ON DELETE SET NULL,
                    open_access boolean NOT NULL DEFAULT true,
                    media_type text,
                    shares jsonb NOT NULL DEFAULT '[]'::jsonb,
                    CONSTRAINT playlists_shares_array
                        CHECK (jsonb_typeof(shares) = 'array'),
                    CONSTRAINT playlists_media_type_nonblank
                        CHECK (media_type IS NULL OR btrim(media_type) <> '')
                );

                CREATE INDEX IF NOT EXISTS playlists_owner_lookup_idx
                    ON jellyfin.playlists (owner_user_id, playlist_id);
                CREATE INDEX IF NOT EXISTS playlists_public_lookup_idx
                    ON jellyfin.playlists (playlist_id) WHERE open_access;
                CREATE INDEX IF NOT EXISTS playlists_shares_gin_idx
                    ON jellyfin.playlists USING gin (shares jsonb_path_ops);
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.playlists CASCADE;")
            .await?;
        Ok(())
    }
}
