use std::collections::{HashMap, HashSet};

use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, Statement, TransactionTrait,
};
use serde_json::{Number, Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::media_stream;

/// Stable database representation of Jellyfin's media-stream type enum.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i16)]
pub enum PersistedMediaStreamType {
    Audio = 0,
    Video = 1,
    Subtitle = 2,
    EmbeddedImage = 3,
    Data = 4,
    Lyric = 5,
}

impl PersistedMediaStreamType {
    pub const ALL: [Self; 6] = [
        Self::Audio,
        Self::Video,
        Self::Subtitle,
        Self::EmbeddedImage,
        Self::Data,
        Self::Lyric,
    ];

    #[must_use]
    pub const fn as_i16(self) -> i16 {
        self as i16
    }
}

impl TryFrom<i16> for PersistedMediaStreamType {
    type Error = InvalidPersistedMediaStreamType;

    fn try_from(value: i16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Audio),
            1 => Ok(Self::Video),
            2 => Ok(Self::Subtitle),
            3 => Ok(Self::EmbeddedImage),
            4 => Ok(Self::Data),
            5 => Ok(Self::Lyric),
            _ => Err(InvalidPersistedMediaStreamType(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("unknown persisted media-stream type {0}")]
pub struct InvalidPersistedMediaStreamType(pub i16);

/// Media-stream fields stored by Jellyfin, excluding derived display metadata.
// The DTO intentionally mirrors Jellyfin's normalized persistence columns.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub struct PersistedMediaStream {
    pub stream_index: i32,
    pub stream_type: PersistedMediaStreamType,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channel_layout: Option<String>,
    pub profile: Option<String>,
    pub aspect_ratio: Option<String>,
    pub path: Option<String>,
    pub is_interlaced: Option<bool>,
    pub bit_rate: Option<i32>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,
    pub is_original: bool,
    pub height: Option<i32>,
    pub width: Option<i32>,
    pub average_frame_rate: Option<f32>,
    pub real_frame_rate: Option<f32>,
    pub level: Option<f32>,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<i32>,
    pub is_anamorphic: Option<bool>,
    pub ref_frames: Option<i32>,
    pub codec_tag: Option<String>,
    pub comment: Option<String>,
    pub nal_length_size: Option<String>,
    pub is_avc: Option<bool>,
    pub title: Option<String>,
    pub time_base: Option<String>,
    pub codec_time_base: Option<String>,
    pub color_range: Option<String>,
    pub color_primaries: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub dv_version_major: Option<i32>,
    pub dv_version_minor: Option<i32>,
    pub dv_profile: Option<i32>,
    pub dv_level: Option<i32>,
    pub rpu_present_flag: Option<i32>,
    pub el_present_flag: Option<i32>,
    pub bl_present_flag: Option<i32>,
    pub dv_bl_signal_compatibility_id: Option<i32>,
    pub is_hearing_impaired: Option<bool>,
    pub rotation: Option<i32>,
    pub hdr10_plus_present_flag: Option<bool>,
}

/// Item-scoped media-stream query matching Jellyfin's repository filters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaStreamQuery {
    pub item_id: Uuid,
    pub stream_index: Option<i32>,
    pub stream_type: Option<PersistedMediaStreamType>,
}

impl MediaStreamQuery {
    #[must_use]
    pub const fn for_item(item_id: Uuid) -> Self {
        Self {
            item_id,
            stream_index: None,
            stream_type: None,
        }
    }
}

/// Media-stream persistence or validation failure.
#[derive(Debug, Error)]
pub enum MediaStreamStoreError {
    #[error("base item {item_id} was not found")]
    BaseItemNotFound { item_id: Uuid },
    #[error("duplicate media-stream index {stream_index}")]
    DuplicateStreamIndex { stream_index: i32 },
    #[error(transparent)]
    InvalidStreamType(#[from] InvalidPersistedMediaStreamType),
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed media-stream storage.
#[derive(Clone)]
pub struct MediaStreamRepository {
    database: crate::SharedDatabase,
}

impl MediaStreamRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Atomically replaces every stream for one item.
    ///
    /// The owning item is locked before stale rows are deleted and the input is
    /// expanded with `jsonb_to_recordset`. Competing replacements therefore
    /// leave one complete set, while `ON CONFLICT` handles stable stream indexes.
    ///
    /// # Errors
    ///
    /// Returns duplicate-index, missing-item, corrupt-row, or database errors.
    pub async fn replace(
        &self,
        item_id: Uuid,
        streams: &[PersistedMediaStream],
    ) -> Result<Vec<PersistedMediaStream>, MediaStreamStoreError> {
        validate_unique_indexes(streams)?;
        let payload = Value::Array(streams.iter().map(PersistedMediaStream::to_json).collect());
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(MediaStreamStoreError::BaseItemNotFound { item_id });
        }

        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH input AS (
                    SELECT stream_index
                    FROM jsonb_to_recordset($2::jsonb) AS stream(stream_index integer)
                )
                DELETE FROM jellyfin.media_streams AS stored
                WHERE stored.item_id = $1
                  AND NOT EXISTS (
                      SELECT 1 FROM input
                      WHERE input.stream_index = stored.stream_index
                  )
                ",
                [item_id.into(), Value::from(payload.as_str()).into()],
            ))
            .await?;

        let rows = media_stream::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            UPSERT_SQL,
            [item_id.into(), payload.into()],
        ))
        .all(&transaction)
        .await?;
        let streams = rows
            .into_iter()
            .map(PersistedMediaStream::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await?;
        Ok(streams)
    }

    /// Queries one item's streams, optionally filtered by index and type.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row or database error.
    pub async fn query(
        &self,
        query: MediaStreamQuery,
    ) -> Result<Vec<PersistedMediaStream>, MediaStreamStoreError> {
        let mut select =
            media_stream::Entity::find().filter(media_stream::Column::ItemId.eq(query.item_id));
        if let Some(stream_index) = query.stream_index {
            select = select.filter(media_stream::Column::StreamIndex.eq(stream_index));
        }
        if let Some(stream_type) = query.stream_type {
            select = select.filter(media_stream::Column::StreamType.eq(stream_type.as_i16()));
        }
        let rows = select
            .order_by_asc(media_stream::Column::StreamIndex)
            .all(self.database.as_ref())
            .await?;
        rows.into_iter()
            .map(PersistedMediaStream::try_from)
            .collect()
    }

    /// Deletes one stream from an item after locking the owning item.
    ///
    /// Missing streams are treated as a successful no-op so API deletes remain
    /// idempotent; a missing owning item still returns not-found.
    ///
    /// # Errors
    ///
    /// Returns missing-item or database errors.
    pub async fn delete_stream(
        &self,
        item_id: Uuid,
        stream_index: i32,
        stream_type: PersistedMediaStreamType,
    ) -> Result<bool, MediaStreamStoreError> {
        let transaction = self.database.begin().await?;
        let owner = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
                [item_id.into()],
            ))
            .await?;
        if owner.is_none() {
            return Err(MediaStreamStoreError::BaseItemNotFound { item_id });
        }

        let result = media_stream::Entity::delete_many()
            .filter(media_stream::Column::ItemId.eq(item_id))
            .filter(media_stream::Column::StreamIndex.eq(stream_index))
            .filter(media_stream::Column::StreamType.eq(stream_type.as_i16()))
            .exec(&transaction)
            .await?;
        transaction.commit().await?;
        Ok(result.rows_affected > 0)
    }

    /// Queries many items' streams in one `PostgreSQL` round-trip.
    ///
    /// Streams are grouped by item id and ordered by stream index inside each
    /// group so callers can project them without extra sorting.
    ///
    /// # Errors
    ///
    /// Returns a corrupt-row or database error.
    pub async fn query_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<PersistedMediaStream>>, MediaStreamStoreError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = media_stream::Entity::find()
            .filter(media_stream::Column::ItemId.is_in(item_ids.iter().copied()))
            .order_by_asc(media_stream::Column::ItemId)
            .order_by_asc(media_stream::Column::StreamIndex)
            .all(self.database.as_ref())
            .await?;

        let mut grouped = HashMap::with_capacity(item_ids.len());
        for row in rows {
            let item_id = row.item_id;
            grouped
                .entry(item_id)
                .or_insert_with(Vec::new)
                .push(PersistedMediaStream::try_from(row)?);
        }
        Ok(grouped)
    }

    /// Returns the distinct languages stored for one stream type.
    ///
    /// Null and empty values map to Jellyfin's `und` language code. Results are
    /// sorted to keep API responses and tests deterministic.
    ///
    /// # Errors
    ///
    /// Returns a database error when the distinct scan fails.
    pub async fn languages(
        &self,
        stream_type: PersistedMediaStreamType,
    ) -> Result<Vec<String>, MediaStreamStoreError> {
        let rows = LanguageRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            SELECT DISTINCT COALESCE(NULLIF(language, ''), 'und') AS language
            FROM jellyfin.media_streams
            WHERE stream_type = $1
            ORDER BY language
            ",
            [stream_type.as_i16().into()],
        ))
        .all(self.database.as_ref())
        .await?;
        Ok(rows.into_iter().map(|row| row.language).collect())
    }
}

#[derive(Debug, FromQueryResult)]
struct LanguageRow {
    language: String,
}

fn validate_unique_indexes(streams: &[PersistedMediaStream]) -> Result<(), MediaStreamStoreError> {
    let mut indexes = HashSet::with_capacity(streams.len());
    for stream in streams {
        if !indexes.insert(stream.stream_index) {
            return Err(MediaStreamStoreError::DuplicateStreamIndex {
                stream_index: stream.stream_index,
            });
        }
    }
    Ok(())
}

impl PersistedMediaStream {
    fn to_json(&self) -> Value {
        let Value::Object(mut object) = json!({
            "stream_index": self.stream_index,
            "stream_type": self.stream_type.as_i16(),
            "codec": self.codec,
            "language": self.language,
            "channel_layout": self.channel_layout,
            "profile": self.profile,
            "aspect_ratio": self.aspect_ratio,
            "path": self.path,
            "is_interlaced": self.is_interlaced,
            "bit_rate": self.bit_rate,
            "channels": self.channels,
            "sample_rate": self.sample_rate,
            "is_default": self.is_default,
            "is_forced": self.is_forced,
            "is_external": self.is_external,
            "is_original": self.is_original,
            "height": self.height,
            "width": self.width,
            "average_frame_rate": float_json(self.average_frame_rate),
            "real_frame_rate": float_json(self.real_frame_rate),
            "level": float_json(self.level),
            "pixel_format": self.pixel_format,
            "bit_depth": self.bit_depth,
            "is_anamorphic": self.is_anamorphic,
            "ref_frames": self.ref_frames,
        }) else {
            unreachable!("a JSON object literal must produce an object");
        };
        let Value::Object(remaining) = json!({
            "codec_tag": self.codec_tag,
            "comment": self.comment,
            "nal_length_size": self.nal_length_size,
            "is_avc": self.is_avc,
            "title": self.title,
            "time_base": self.time_base,
            "codec_time_base": self.codec_time_base,
            "color_range": self.color_range,
            "color_primaries": self.color_primaries,
            "color_space": self.color_space,
            "color_transfer": self.color_transfer,
            "dv_version_major": self.dv_version_major,
            "dv_version_minor": self.dv_version_minor,
            "dv_profile": self.dv_profile,
            "dv_level": self.dv_level,
            "rpu_present_flag": self.rpu_present_flag,
            "el_present_flag": self.el_present_flag,
            "bl_present_flag": self.bl_present_flag,
            "dv_bl_signal_compatibility_id": self.dv_bl_signal_compatibility_id,
            "is_hearing_impaired": self.is_hearing_impaired,
            "rotation": self.rotation,
            "hdr10_plus_present_flag": self.hdr10_plus_present_flag,
        }) else {
            unreachable!("a JSON object literal must produce an object");
        };
        object.extend(remaining);
        Value::Object(object)
    }
}

fn float_json(value: Option<f32>) -> Value {
    match value {
        None => Value::Null,
        Some(value) if value.is_nan() => Value::String("NaN".to_owned()),
        Some(value) if value == f32::INFINITY => Value::String("Infinity".to_owned()),
        Some(value) if value == f32::NEG_INFINITY => Value::String("-Infinity".to_owned()),
        Some(value) => Number::from_f64(f64::from(value)).map_or(Value::Null, Value::Number),
    }
}

impl TryFrom<media_stream::Model> for PersistedMediaStream {
    type Error = MediaStreamStoreError;

    fn try_from(row: media_stream::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            stream_index: row.stream_index,
            stream_type: row.stream_type.try_into()?,
            codec: row.codec,
            language: row.language,
            channel_layout: row.channel_layout,
            profile: row.profile,
            aspect_ratio: row.aspect_ratio,
            path: row.path,
            is_interlaced: row.is_interlaced,
            bit_rate: row.bit_rate,
            channels: row.channels,
            sample_rate: row.sample_rate,
            is_default: row.is_default,
            is_forced: row.is_forced,
            is_external: row.is_external,
            is_original: row.is_original,
            height: row.height,
            width: row.width,
            average_frame_rate: row.average_frame_rate,
            real_frame_rate: row.real_frame_rate,
            level: row.level,
            pixel_format: row.pixel_format,
            bit_depth: row.bit_depth,
            is_anamorphic: row.is_anamorphic,
            ref_frames: row.ref_frames,
            codec_tag: row.codec_tag,
            comment: row.comment,
            nal_length_size: row.nal_length_size,
            is_avc: row.is_avc,
            title: row.title,
            time_base: row.time_base,
            codec_time_base: row.codec_time_base,
            color_range: row.color_range,
            color_primaries: row.color_primaries,
            color_space: row.color_space,
            color_transfer: row.color_transfer,
            dv_version_major: row.dv_version_major,
            dv_version_minor: row.dv_version_minor,
            dv_profile: row.dv_profile,
            dv_level: row.dv_level,
            rpu_present_flag: row.rpu_present_flag,
            el_present_flag: row.el_present_flag,
            bl_present_flag: row.bl_present_flag,
            dv_bl_signal_compatibility_id: row.dv_bl_signal_compatibility_id,
            is_hearing_impaired: row.is_hearing_impaired,
            rotation: row.rotation,
            hdr10_plus_present_flag: row.hdr10_plus_present_flag,
        })
    }
}

const UPSERT_SQL: &str = r"
    WITH input AS (
        SELECT *
        FROM jsonb_to_recordset($2::jsonb) AS stream(
            stream_index integer, stream_type smallint, codec text, language text,
            channel_layout text, profile text, aspect_ratio text, path text,
            is_interlaced boolean, bit_rate integer, channels integer, sample_rate integer,
            is_default boolean, is_forced boolean, is_external boolean, is_original boolean,
            height integer, width integer, average_frame_rate real, real_frame_rate real,
            level real, pixel_format text, bit_depth integer, is_anamorphic boolean,
            ref_frames integer, codec_tag text, comment text, nal_length_size text,
            is_avc boolean, title text, time_base text, codec_time_base text,
            color_range text, color_primaries text, color_space text, color_transfer text,
            dv_version_major integer, dv_version_minor integer, dv_profile integer,
            dv_level integer, rpu_present_flag integer, el_present_flag integer,
            bl_present_flag integer, dv_bl_signal_compatibility_id integer,
            is_hearing_impaired boolean, rotation integer, hdr10_plus_present_flag boolean
        )
    ), upserted AS (
        INSERT INTO jellyfin.media_streams (
            item_id, stream_index, stream_type, codec, language, channel_layout, profile,
            aspect_ratio, path, is_interlaced, bit_rate, channels, sample_rate,
            is_default, is_forced, is_external, is_original, height, width,
            average_frame_rate, real_frame_rate, level, pixel_format, bit_depth,
            is_anamorphic, ref_frames, codec_tag, comment, nal_length_size, is_avc,
            title, time_base, codec_time_base, color_range, color_primaries, color_space,
            color_transfer, dv_version_major, dv_version_minor, dv_profile, dv_level,
            rpu_present_flag, el_present_flag, bl_present_flag,
            dv_bl_signal_compatibility_id, is_hearing_impaired, rotation,
            hdr10_plus_present_flag
        )
        SELECT
            $1, stream_index, stream_type, codec, language, channel_layout, profile,
            aspect_ratio, path, is_interlaced, bit_rate, channels, sample_rate,
            is_default, is_forced, is_external, is_original, height, width,
            average_frame_rate, real_frame_rate, level, pixel_format, bit_depth,
            is_anamorphic, ref_frames, codec_tag, comment, nal_length_size, is_avc,
            title, time_base, codec_time_base, color_range, color_primaries, color_space,
            color_transfer, dv_version_major, dv_version_minor, dv_profile, dv_level,
            rpu_present_flag, el_present_flag, bl_present_flag,
            dv_bl_signal_compatibility_id, is_hearing_impaired, rotation,
            hdr10_plus_present_flag
        FROM input
        ON CONFLICT (item_id, stream_index) DO UPDATE SET
            (stream_type, codec, language, channel_layout, profile, aspect_ratio, path,
             is_interlaced, bit_rate, channels, sample_rate, is_default, is_forced,
             is_external, is_original, height, width, average_frame_rate, real_frame_rate,
             level, pixel_format, bit_depth, is_anamorphic, ref_frames, codec_tag,
             comment, nal_length_size, is_avc, title, time_base, codec_time_base,
             color_range, color_primaries, color_space, color_transfer, dv_version_major,
             dv_version_minor, dv_profile, dv_level, rpu_present_flag, el_present_flag,
             bl_present_flag, dv_bl_signal_compatibility_id, is_hearing_impaired,
             rotation, hdr10_plus_present_flag) =
            (EXCLUDED.stream_type, EXCLUDED.codec, EXCLUDED.language,
             EXCLUDED.channel_layout, EXCLUDED.profile, EXCLUDED.aspect_ratio,
             EXCLUDED.path, EXCLUDED.is_interlaced, EXCLUDED.bit_rate,
             EXCLUDED.channels, EXCLUDED.sample_rate, EXCLUDED.is_default,
             EXCLUDED.is_forced, EXCLUDED.is_external, EXCLUDED.is_original,
             EXCLUDED.height, EXCLUDED.width, EXCLUDED.average_frame_rate,
             EXCLUDED.real_frame_rate, EXCLUDED.level, EXCLUDED.pixel_format,
             EXCLUDED.bit_depth, EXCLUDED.is_anamorphic, EXCLUDED.ref_frames,
             EXCLUDED.codec_tag, EXCLUDED.comment, EXCLUDED.nal_length_size,
             EXCLUDED.is_avc, EXCLUDED.title, EXCLUDED.time_base,
             EXCLUDED.codec_time_base, EXCLUDED.color_range, EXCLUDED.color_primaries,
             EXCLUDED.color_space, EXCLUDED.color_transfer, EXCLUDED.dv_version_major,
             EXCLUDED.dv_version_minor, EXCLUDED.dv_profile, EXCLUDED.dv_level,
             EXCLUDED.rpu_present_flag, EXCLUDED.el_present_flag,
             EXCLUDED.bl_present_flag, EXCLUDED.dv_bl_signal_compatibility_id,
             EXCLUDED.is_hearing_impaired, EXCLUDED.rotation,
             EXCLUDED.hdr10_plus_present_flag)
        RETURNING *
    )
    SELECT * FROM upserted ORDER BY stream_index
";
