use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    #[allow(clippy::needless_raw_string_hashes)]
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE jellyfin.server_configuration
                    ADD COLUMN IF NOT EXISTS metadata_options jsonb NOT NULL DEFAULT
                    '[
                        {"ItemType":"Book","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":[],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"Movie","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":[],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"MusicVideo","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":["The Open Movie Database"],"MetadataFetcherOrder":[],"DisabledImageFetchers":["The Open Movie Database"],"ImageFetcherOrder":[]},
                        {"ItemType":"Series","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":[],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"MusicAlbum","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":["TheAudioDB"],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"MusicArtist","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":["TheAudioDB"],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"BoxSet","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":[],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"Season","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":[],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]},
                        {"ItemType":"Episode","DisabledMetadataSavers":[],"LocalMetadataReaderOrder":[],"DisabledMetadataFetchers":[],"MetadataFetcherOrder":[],"DisabledImageFetchers":[],"ImageFetcherOrder":[]}
                    ]'::jsonb;

                DO $$
                BEGIN
                    IF NOT EXISTS (
                        SELECT 1
                        FROM pg_constraint
                        WHERE connamespace = 'jellyfin'::regnamespace
                          AND conrelid = 'jellyfin.server_configuration'::regclass
                          AND conname = 'server_configuration_metadata_options_array'
                    ) THEN
                        ALTER TABLE jellyfin.server_configuration
                            ADD CONSTRAINT server_configuration_metadata_options_array
                            CHECK (jsonb_typeof(metadata_options) = 'array');
                    END IF;
                END
                $$;
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.server_configuration \
                 DROP COLUMN IF EXISTS metadata_options;",
            )
            .await?;
        Ok(())
    }
}
