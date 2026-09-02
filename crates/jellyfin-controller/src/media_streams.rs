use std::collections::HashMap;

use jellyfin_data::{
    MediaStreamQuery as PersistedMediaStreamQuery, MediaStreamRepository, MediaStreamStoreError,
    PersistedMediaStream, PersistedMediaStreamType,
};
use jellyfin_model::{MediaStream, MediaStreamType};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::LocalizationService;

const DEFAULT_SERVER_CULTURE: &str = "en-US";

/// Item-scoped media-stream query in the API model vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaStreamFilter {
    pub item_id: Uuid,
    pub index: Option<i32>,
    pub stream_type: Option<MediaStreamType>,
}

impl MediaStreamFilter {
    #[must_use]
    pub const fn for_item(item_id: Uuid) -> Self {
        Self {
            item_id,
            index: None,
            stream_type: None,
        }
    }
}

/// Converts external stream paths between persisted and API-facing forms.
pub trait MediaStreamPathMapper: Clone + Send + Sync + 'static {
    fn path_to_save(&self, path: &str) -> Option<String>;
    fn restore_path(&self, path: &str) -> Option<String>;
}

/// Identity path mapping used until the server host owns virtual-path expansion.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IdentityMediaStreamPathMapper;

impl MediaStreamPathMapper for IdentityMediaStreamPathMapper {
    fn path_to_save(&self, path: &str) -> Option<String> {
        Some(path.to_owned())
    }

    fn restore_path(&self, path: &str) -> Option<String> {
        Some(path.to_owned())
    }
}

/// Projects persisted media-stream rows to Jellyfin API DTOs and back.
#[derive(Debug, Clone)]
pub struct MediaStreamMapper<P = IdentityMediaStreamPathMapper> {
    path_mapper: P,
    server_culture: String,
}

impl Default for MediaStreamMapper<IdentityMediaStreamPathMapper> {
    fn default() -> Self {
        Self::new(IdentityMediaStreamPathMapper, DEFAULT_SERVER_CULTURE)
    }
}

impl<P> MediaStreamMapper<P>
where
    P: MediaStreamPathMapper,
{
    #[must_use]
    pub fn new(path_mapper: P, server_culture: impl Into<String>) -> Self {
        Self {
            path_mapper,
            server_culture: server_culture.into(),
        }
    }

    #[must_use]
    pub fn to_api(&self, persisted: PersistedMediaStream) -> MediaStream {
        let localization = LocalizationService;
        let language = persisted.language.map(|language| {
            localization
                .try_get_iso6392_t_from_b(&language)
                .map(str::to_owned)
                .unwrap_or(language)
        });
        let path = restore_path(&self.path_mapper, persisted.path);
        let mut stream = MediaStream {
            codec: persisted.codec,
            codec_tag: persisted.codec_tag,
            language,
            color_range: persisted.color_range,
            color_space: persisted.color_space,
            color_transfer: persisted.color_transfer,
            color_primaries: persisted.color_primaries,
            dv_version_major: persisted.dv_version_major,
            dv_version_minor: persisted.dv_version_minor,
            dv_profile: persisted.dv_profile,
            dv_level: persisted.dv_level,
            rpu_present_flag: persisted.rpu_present_flag,
            el_present_flag: persisted.el_present_flag,
            bl_present_flag: persisted.bl_present_flag,
            dv_bl_signal_compatibility_id: persisted.dv_bl_signal_compatibility_id,
            comment: persisted.comment,
            time_base: persisted.time_base,
            codec_time_base: persisted.codec_time_base,
            title: persisted.title,
            hdr10_plus_present_flag: persisted.hdr10_plus_present_flag,
            nal_length_size: persisted.nal_length_size,
            is_interlaced: persisted.is_interlaced.unwrap_or_default(),
            is_avc: persisted.is_avc,
            channel_layout: persisted.channel_layout,
            channels: persisted.channels,
            bit_rate: persisted.bit_rate,
            bit_depth: persisted.bit_depth,
            sample_rate: persisted.sample_rate,
            index: persisted.stream_index,
            is_default: persisted.is_default,
            is_forced: persisted.is_forced,
            is_hearing_impaired: persisted.is_hearing_impaired.unwrap_or_default(),
            is_original: persisted.is_original,
            height: persisted.height,
            width: persisted.width,
            average_frame_rate: persisted.average_frame_rate,
            real_frame_rate: persisted.real_frame_rate,
            profile: persisted.profile,
            aspect_ratio: persisted.aspect_ratio,
            level: persisted.level.map(f64::from),
            ref_frames: persisted.ref_frames,
            rotation: persisted.rotation,
            pixel_format: persisted.pixel_format,
            is_anamorphic: persisted.is_anamorphic,
            stream_type: persisted_type_to_model(persisted.stream_type),
            is_external: persisted.is_external,
            path,
            ..MediaStream::default()
        };
        apply_localization(&mut stream, localization, &self.server_culture);
        stream.video_range_type = stream.video_range_type();
        stream
    }

    #[must_use]
    pub fn to_persisted(&self, mut stream: MediaStream) -> PersistedMediaStream {
        let path = save_path(&self.path_mapper, stream.path.as_deref());
        PersistedMediaStream {
            stream_index: stream.index,
            stream_type: model_type_to_persisted(stream.stream_type),
            codec: stream.codec.take(),
            language: stream.language.take(),
            channel_layout: stream.channel_layout.take(),
            profile: stream.profile.take(),
            aspect_ratio: stream.aspect_ratio.take(),
            path,
            is_interlaced: Some(stream.is_interlaced),
            bit_rate: stream.bit_rate,
            channels: stream.channels,
            sample_rate: stream.sample_rate,
            is_default: stream.is_default,
            is_forced: stream.is_forced,
            is_external: stream.is_external,
            is_original: stream.is_original,
            height: stream.height,
            width: stream.width,
            average_frame_rate: stream.average_frame_rate,
            real_frame_rate: stream.real_frame_rate,
            level: stream.level.map(f64_to_f32),
            pixel_format: stream.pixel_format.take(),
            bit_depth: stream.bit_depth,
            is_anamorphic: stream.is_anamorphic,
            ref_frames: stream.ref_frames,
            codec_tag: stream.codec_tag.take(),
            comment: stream.comment.take(),
            nal_length_size: stream.nal_length_size.take(),
            is_avc: stream.is_avc,
            title: stream.title.take(),
            time_base: stream.time_base.take(),
            codec_time_base: stream.codec_time_base.take(),
            color_range: stream.color_range.take(),
            color_primaries: stream.color_primaries.take(),
            color_space: stream.color_space.take(),
            color_transfer: stream.color_transfer.take(),
            dv_version_major: stream.dv_version_major,
            dv_version_minor: stream.dv_version_minor,
            dv_profile: stream.dv_profile,
            dv_level: stream.dv_level,
            rpu_present_flag: stream.rpu_present_flag,
            el_present_flag: stream.el_present_flag,
            bl_present_flag: stream.bl_present_flag,
            dv_bl_signal_compatibility_id: stream.dv_bl_signal_compatibility_id,
            is_hearing_impaired: Some(stream.is_hearing_impaired),
            rotation: stream.rotation,
            hdr10_plus_present_flag: stream.hdr10_plus_present_flag,
        }
    }
}

/// API-facing service for persisted media-stream metadata.
#[derive(Clone)]
pub struct MediaStreamService<P = IdentityMediaStreamPathMapper> {
    repository: MediaStreamRepository,
    mapper: MediaStreamMapper<P>,
}

impl MediaStreamService<IdentityMediaStreamPathMapper> {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self::with_mapper(database, MediaStreamMapper::default())
    }
}

impl<P> MediaStreamService<P>
where
    P: MediaStreamPathMapper,
{
    #[must_use]
    pub fn with_mapper(database: DatabaseConnection, mapper: MediaStreamMapper<P>) -> Self {
        Self {
            repository: MediaStreamRepository::new(database),
            mapper,
        }
    }

    /// Replaces one item's persisted streams and returns API-projected rows.
    ///
    /// # Errors
    ///
    /// Returns validation or `PostgreSQL` persistence errors.
    pub async fn save_media_streams(
        &self,
        item_id: Uuid,
        streams: Vec<MediaStream>,
    ) -> Result<Vec<MediaStream>, MediaStreamServiceError> {
        let persisted = streams
            .into_iter()
            .map(|stream| self.mapper.to_persisted(stream))
            .collect::<Vec<_>>();
        let stored = self.repository.replace(item_id, &persisted).await?;
        Ok(stored
            .into_iter()
            .map(|stream| self.mapper.to_api(stream))
            .collect())
    }

    /// Queries persisted media streams using Jellyfin's item/index/type filters.
    ///
    /// # Errors
    ///
    /// Returns `PostgreSQL` persistence errors.
    pub async fn get_media_streams(
        &self,
        filter: MediaStreamFilter,
    ) -> Result<Vec<MediaStream>, MediaStreamServiceError> {
        let query = PersistedMediaStreamQuery {
            item_id: filter.item_id,
            stream_index: filter.index,
            stream_type: filter.stream_type.map(model_type_to_persisted),
        };
        let stored = self.repository.query(query).await?;
        Ok(stored
            .into_iter()
            .map(|stream| self.mapper.to_api(stream))
            .collect())
    }

    /// Deletes one item-scoped media stream by index and type.
    ///
    /// # Errors
    ///
    /// Returns `PostgreSQL` persistence errors.
    pub async fn delete_media_stream(
        &self,
        item_id: Uuid,
        index: i32,
        stream_type: MediaStreamType,
    ) -> Result<bool, MediaStreamServiceError> {
        Ok(self
            .repository
            .delete_stream(item_id, index, model_type_to_persisted(stream_type))
            .await?)
    }

    /// Queries many items' streams in one database round-trip.
    ///
    /// # Errors
    ///
    /// Returns `PostgreSQL` persistence errors.
    pub async fn get_media_streams_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<MediaStream>>, MediaStreamServiceError> {
        let stored = self.repository.query_for_items(item_ids).await?;
        Ok(stored
            .into_iter()
            .map(|(item_id, streams)| {
                (
                    item_id,
                    streams
                        .into_iter()
                        .map(|stream| self.mapper.to_api(stream))
                        .collect(),
                )
            })
            .collect())
    }

    /// Returns distinct language codes for a persisted stream type.
    ///
    /// # Errors
    ///
    /// Returns `PostgreSQL` persistence errors.
    pub async fn get_media_stream_languages(
        &self,
        stream_type: MediaStreamType,
    ) -> Result<Vec<String>, MediaStreamServiceError> {
        Ok(self
            .repository
            .languages(model_type_to_persisted(stream_type))
            .await?)
    }
}

#[derive(Debug, Error)]
pub enum MediaStreamServiceError {
    #[error(transparent)]
    Store(#[from] MediaStreamStoreError),
}

fn apply_localization(
    stream: &mut MediaStream,
    localization: LocalizationService,
    server_culture: &str,
) {
    if !matches!(
        stream.stream_type,
        MediaStreamType::Audio | MediaStreamType::Subtitle
    ) {
        return;
    }

    stream.localized_default =
        Some(localization.server_localized_string("Default", server_culture));
    stream.localized_external =
        Some(localization.server_localized_string("External", server_culture));
    stream.localized_language = localization.language_display_name(stream.language.as_deref());

    match stream.stream_type {
        MediaStreamType::Audio => {
            stream.localized_original =
                Some(localization.server_localized_string("Original", server_culture));
        }
        MediaStreamType::Subtitle => {
            stream.localized_undefined =
                Some(localization.server_localized_string("Undefined", server_culture));
            stream.localized_forced =
                Some(localization.server_localized_string("Forced", server_culture));
            stream.localized_hearing_impaired =
                Some(localization.server_localized_string("HearingImpaired", server_culture));
        }
        MediaStreamType::Video
        | MediaStreamType::EmbeddedImage
        | MediaStreamType::Data
        | MediaStreamType::Lyric => {}
    }
}

fn save_path<P>(mapper: &P, path: Option<&str>) -> Option<String>
where
    P: MediaStreamPathMapper,
{
    path.map(|path| mapper.path_to_save(path).unwrap_or_else(|| path.to_owned()))
}

fn restore_path<P>(mapper: &P, path: Option<String>) -> Option<String>
where
    P: MediaStreamPathMapper,
{
    path.map(|path| mapper.restore_path(&path).unwrap_or(path))
}

const fn persisted_type_to_model(value: PersistedMediaStreamType) -> MediaStreamType {
    match value {
        PersistedMediaStreamType::Audio => MediaStreamType::Audio,
        PersistedMediaStreamType::Video => MediaStreamType::Video,
        PersistedMediaStreamType::Subtitle => MediaStreamType::Subtitle,
        PersistedMediaStreamType::EmbeddedImage => MediaStreamType::EmbeddedImage,
        PersistedMediaStreamType::Data => MediaStreamType::Data,
        PersistedMediaStreamType::Lyric => MediaStreamType::Lyric,
    }
}

const fn model_type_to_persisted(value: MediaStreamType) -> PersistedMediaStreamType {
    match value {
        MediaStreamType::Audio => PersistedMediaStreamType::Audio,
        MediaStreamType::Video => PersistedMediaStreamType::Video,
        MediaStreamType::Subtitle => PersistedMediaStreamType::Subtitle,
        MediaStreamType::EmbeddedImage => PersistedMediaStreamType::EmbeddedImage,
        MediaStreamType::Data => PersistedMediaStreamType::Data,
        MediaStreamType::Lyric => PersistedMediaStreamType::Lyric,
    }
}

#[allow(clippy::cast_possible_truncation)]
fn f64_to_f32(value: f64) -> f32 {
    value as f32
}
