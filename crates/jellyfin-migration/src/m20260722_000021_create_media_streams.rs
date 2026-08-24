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
                CREATE TABLE IF NOT EXISTS jellyfin.media_streams (
                    item_id uuid NOT NULL,
                    stream_index integer NOT NULL,
                    stream_type smallint NOT NULL,
                    codec text,
                    language text,
                    channel_layout text,
                    profile text,
                    aspect_ratio text,
                    path text,
                    is_interlaced boolean,
                    bit_rate integer,
                    channels integer,
                    sample_rate integer,
                    is_default boolean NOT NULL,
                    is_forced boolean NOT NULL,
                    is_external boolean NOT NULL,
                    is_original boolean NOT NULL,
                    height integer,
                    width integer,
                    average_frame_rate real,
                    real_frame_rate real,
                    level real,
                    pixel_format text,
                    bit_depth integer,
                    is_anamorphic boolean,
                    ref_frames integer,
                    codec_tag text,
                    comment text,
                    nal_length_size text,
                    is_avc boolean,
                    title text,
                    time_base text,
                    codec_time_base text,
                    color_range text,
                    color_primaries text,
                    color_space text,
                    color_transfer text,
                    dv_version_major integer,
                    dv_version_minor integer,
                    dv_profile integer,
                    dv_level integer,
                    rpu_present_flag integer,
                    el_present_flag integer,
                    bl_present_flag integer,
                    dv_bl_signal_compatibility_id integer,
                    is_hearing_impaired boolean,
                    rotation integer,
                    hdr10_plus_present_flag boolean,
                    CONSTRAINT media_streams_pkey
                        PRIMARY KEY (item_id, stream_index),
                    CONSTRAINT media_streams_item_id_fkey
                        FOREIGN KEY (item_id)
                        REFERENCES jellyfin.base_items (id)
                        ON DELETE CASCADE,
                    CONSTRAINT media_streams_type_valid
                        CHECK (stream_type BETWEEN 0 AND 5)
                );

                COMMENT ON TABLE jellyfin.media_streams IS
                    'Normalized media-stream probe data. The item/index primary key '
                    'supports item-scoped ordered reads without the write amplification '
                    'of the legacy language and type indexes removed upstream.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.media_streams CASCADE;")
            .await?;
        Ok(())
    }
}
