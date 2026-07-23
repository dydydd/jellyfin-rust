use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_controller::{
    Genre, LocalizationService, MusicGenre, Person, RelatedItemKind, Year,
    library::get_media_source_name,
};
use jellyfin_data::entities::{base_item, user_data};
use jellyfin_model::{
    MediaAttachment, MediaProtocol, MediaSourceInfo, MediaSourceType, MediaStream, MediaStreamType,
    SubtitlePlaybackMode, UserConfiguration,
};
use jellyfin_server_implementations::MediaStreamSelector;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserIdQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BaseItemDtoFields {
    media_sources: bool,
    media_streams: bool,
}

impl BaseItemDtoFields {
    #[must_use]
    pub(crate) fn from_names(fields: &[String]) -> Self {
        let mut result = Self::default();
        for field in fields {
            if field.eq_ignore_ascii_case("MediaSources") {
                result.media_sources = true;
            } else if field.eq_ignore_ascii_case("MediaStreams") {
                result.media_streams = true;
            }
        }
        result
    }

    #[must_use]
    pub(crate) const fn wants_media_streams(self) -> bool {
        self.media_sources || self.media_streams
    }

    #[must_use]
    pub(crate) const fn wants_media_attachments(self) -> bool {
        self.media_sources
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    pub name: Option<String>,
    pub server_id: String,
    pub id: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub etag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_type: Option<String>,
    pub is_folder: bool,
    pub is_virtual_item: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_unique_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_lyrics: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_sources: Option<Vec<jellyfin_model::MediaSourceInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_streams: Option<Vec<jellyfin_model::MediaStream>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemQueryResult {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: usize,
    pub start_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MediaStreamDefaults {
    audio_preference: AudioLanguagePreference,
    subtitle_languages: Vec<String>,
    play_default_audio_track: bool,
    subtitle_mode: SubtitlePlaybackMode,
    remembered_selections: RememberedStreamSelections,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AudioLanguagePreference {
    Languages(Vec<String>),
    OriginalLanguage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RememberedStreamSelections {
    audio: bool,
    subtitle: bool,
}

impl MediaStreamDefaults {
    #[must_use]
    pub(crate) fn from_user_configuration(configuration: &UserConfiguration) -> Self {
        let prefer_original_audio = configuration
            .audio_language_preference
            .as_deref()
            .is_some_and(|language| language.eq_ignore_ascii_case("OriginalLanguage"));
        Self {
            audio_preference: if prefer_original_audio {
                AudioLanguagePreference::OriginalLanguage
            } else {
                AudioLanguagePreference::Languages(normalize_language(
                    configuration.audio_language_preference.as_deref(),
                ))
            },
            subtitle_languages: normalize_language(
                configuration.subtitle_language_preference.as_deref(),
            ),
            play_default_audio_track: configuration.play_default_audio_track,
            subtitle_mode: configuration.subtitle_mode,
            remembered_selections: RememberedStreamSelections {
                audio: configuration.remember_audio_selections,
                subtitle: configuration.remember_subtitle_selections,
            },
        }
    }
}

pub(crate) async fn media_stream_defaults_for_user(
    state: &AppState,
    user_id: Uuid,
    fields: BaseItemDtoFields,
) -> Result<Option<MediaStreamDefaults>, ApiError> {
    if !fields.wants_media_streams() {
        return Ok(None);
    }

    let user = state.users.get(user_id).await?;
    let configuration: UserConfiguration =
        serde_json::from_value(user.preferences).unwrap_or_default();
    Ok(Some(MediaStreamDefaults::from_user_configuration(
        &configuration,
    )))
}

pub(crate) async fn get_root_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_root_for(
        state,
        headers,
        Some(user_id),
        BaseItemDtoFields::from_names(&query.fields),
    )
    .await
}

pub(crate) async fn get_root(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_root_for(
        state,
        headers,
        query.user_id,
        BaseItemDtoFields::from_names(&query.fields),
    )
    .await
}

pub(crate) async fn get_item_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_item_for(
        state,
        headers,
        Some(user_id),
        item_id,
        BaseItemDtoFields::from_names(&query.fields),
    )
    .await
}

pub(crate) async fn get_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_item_for(
        state,
        headers,
        query.user_id,
        item_id,
        BaseItemDtoFields::from_names(&query.fields),
    )
    .await
}

pub(crate) async fn get_intros_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    get_related_query_for(
        state,
        headers,
        Some(user_id),
        item_id,
        RelatedItemKind::Intro,
    )
    .await
}

pub(crate) async fn get_intros(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    get_related_query_for(
        state,
        headers,
        query.user_id,
        item_id,
        RelatedItemKind::Intro,
    )
    .await
}

pub(crate) async fn get_local_trailers_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        Some(user_id),
        item_id,
        RelatedItemKind::LocalTrailer,
    )
    .await
}

pub(crate) async fn get_local_trailers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        query.user_id,
        item_id,
        RelatedItemKind::LocalTrailer,
    )
    .await
}

pub(crate) async fn get_special_features_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        Some(user_id),
        item_id,
        RelatedItemKind::SpecialFeature,
    )
    .await
}

pub(crate) async fn get_special_features(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        query.user_id,
        item_id,
        RelatedItemKind::SpecialFeature,
    )
    .await
}

pub(crate) async fn get_lyrics_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    get_lyrics_for(state, headers, Some(user_id), item_id).await
}

pub(crate) async fn get_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    get_lyrics_for(state, headers, None, item_id).await
}

async fn get_root_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    requested_fields: BaseItemDtoFields,
) -> Result<Json<BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let item = state
        .user_library
        .root(&authenticated.user, target_user_id)
        .await?;
    let defaults =
        media_stream_defaults_for_user(state.as_ref(), target_user_id, requested_fields).await?;
    let remembered_user_data =
        preferred_user_data_for_item(state.as_ref(), target_user_id, &item, requested_fields)
            .await?;
    Ok(Json(
        project_item_to_dto(
            state.as_ref(),
            item,
            requested_fields,
            defaults.as_ref(),
            remembered_user_data.as_ref(),
        )
        .await?,
    ))
}

async fn get_item_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    requested_fields: BaseItemDtoFields,
) -> Result<Json<BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let item = state
        .user_library
        .item(&authenticated.user, target_user_id, item_id)
        .await?;
    let defaults =
        media_stream_defaults_for_user(state.as_ref(), target_user_id, requested_fields).await?;
    let remembered_user_data =
        preferred_user_data_for_item(state.as_ref(), target_user_id, &item, requested_fields)
            .await?;
    Ok(Json(
        project_item_to_dto(
            state.as_ref(),
            item,
            requested_fields,
            defaults.as_ref(),
            remembered_user_data.as_ref(),
        )
        .await?,
    ))
}

async fn get_related_query_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    kind: RelatedItemKind,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let items = related_items(state.clone(), headers, requested_user_id, item_id, kind).await?;
    let items = items
        .into_iter()
        .map(|item| item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}

async fn get_related_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    kind: RelatedItemKind,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    let items = related_items(state.clone(), headers, requested_user_id, item_id, kind).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|item| item_to_dto(item, state.server_id()))
            .collect(),
    ))
}

async fn related_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    kind: RelatedItemKind,
) -> Result<Vec<base_item::Model>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    Ok(state
        .user_library
        .related_items(&authenticated.user, target_user_id, item_id, kind)
        .await?)
}

async fn get_lyrics_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Json<Value>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let lyrics = state
        .user_library
        .lyrics(&authenticated.user, target_user_id, item_id)
        .await?;
    Ok(Json(lyrics))
}

pub(crate) fn item_to_dto(item: base_item::Model, server_id: &str) -> BaseItemDto {
    let extra_type = metadata_string(item.data.as_ref(), &["ExtraType", "extra_type"]);
    let has_lyrics = item
        .data
        .as_ref()
        .and_then(Value::as_object)
        .map(|object| object.contains_key("Lyrics") || object.contains_key("lyrics"));
    BaseItemDto {
        name: item.name,
        server_id: server_id.to_owned(),
        id: item.id.simple().to_string(),
        item_type: item.item_type,
        etag: item.row_version.to_string(),
        date_created: Some(item.date_created.to_rfc3339()),
        sort_name: item.sort_name,
        path: item.path,
        overview: item.overview,
        media_type: item.media_type,
        collection_type: None,
        is_folder: item.is_folder,
        is_virtual_item: item.is_virtual_item,
        parent_id: item.parent_id.map(|id| id.simple().to_string()),
        index_number: item.index_number,
        parent_index_number: item.parent_index_number,
        production_year: item.production_year,
        run_time_ticks: item.runtime_ticks,
        presentation_unique_key: item.presentation_unique_key,
        series_id: item.series_id.map(|id| id.simple().to_string()),
        season_id: item.season_id.map(|id| id.simple().to_string()),
        extra_type,
        has_lyrics,
        provider_ids: metadata_value(item.data.as_ref(), &["ProviderIds", "provider_ids"]),
        media_sources: None,
        media_streams: None,
    }
}

pub(crate) async fn project_item_to_dto(
    state: &AppState,
    item: base_item::Model,
    fields: BaseItemDtoFields,
    defaults: Option<&MediaStreamDefaults>,
    remembered_user_data: Option<&user_data::Model>,
) -> Result<BaseItemDto, ApiError> {
    let item_id = item.id;
    let original_language = original_language_from_item(&item);
    let mut dto = item_to_dto(item, state.server_id());
    if !fields.wants_media_streams() {
        return Ok(dto);
    }

    let media_streams = state
        .media_streams
        .get_media_streams_for_items(&[item_id])
        .await?
        .remove(&item_id)
        .unwrap_or_default();
    let media_attachments = if fields.wants_media_attachments() {
        state
            .media_attachments
            .get_media_attachments_for_items(&[item_id])
            .await?
            .remove(&item_id)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    project_item_dto_with_streams(
        &mut dto,
        fields,
        media_streams,
        media_attachments,
        defaults,
        remembered_user_data,
        original_language.as_deref(),
    );
    Ok(dto)
}

pub(crate) fn project_item_dto_with_streams(
    dto: &mut BaseItemDto,
    fields: BaseItemDtoFields,
    mut media_streams: Vec<MediaStream>,
    media_attachments: Vec<MediaAttachment>,
    defaults: Option<&MediaStreamDefaults>,
    remembered_user_data: Option<&user_data::Model>,
    original_language: Option<&str>,
) {
    let (default_audio_stream_index, default_subtitle_stream_index) = apply_media_stream_defaults(
        dto,
        &mut media_streams,
        defaults,
        remembered_user_data,
        original_language,
    );

    if fields.media_sources
        && let Some(source) = media_source_from_dto(
            dto,
            &media_streams,
            media_attachments,
            default_audio_stream_index,
            default_subtitle_stream_index,
        )
    {
        dto.media_sources = Some(vec![source]);
    }
    if fields.media_streams {
        dto.media_streams = Some(media_streams);
    }
}

fn media_source_from_dto(
    dto: &BaseItemDto,
    media_streams: &[MediaStream],
    media_attachments: Vec<MediaAttachment>,
    default_audio_stream_index: Option<i32>,
    default_subtitle_stream_index: Option<i32>,
) -> Option<MediaSourceInfo> {
    if dto.is_folder || !is_media_source_item(dto) {
        return None;
    }

    let path = dto.path.clone();
    let name = path
        .as_deref()
        .map(|path| get_media_source_name(path, false, None))
        .or_else(|| dto.name.clone());
    let container = path.as_deref().and_then(media_container_from_path);
    Some(MediaSourceInfo {
        id: Some(dto.id.clone()),
        protocol: MediaProtocol::File,
        path,
        name,
        container,
        source_type: MediaSourceType::Default,
        run_time_ticks: dto.run_time_ticks,
        media_streams: media_streams.to_vec(),
        media_attachments,
        default_audio_stream_index,
        default_subtitle_stream_index,
        ..MediaSourceInfo::default()
    })
}

fn apply_media_stream_defaults(
    dto: &BaseItemDto,
    media_streams: &mut [MediaStream],
    defaults: Option<&MediaStreamDefaults>,
    remembered_user_data: Option<&user_data::Model>,
    original_language: Option<&str>,
) -> (Option<i32>, Option<i32>) {
    let Some(defaults) = defaults else {
        return (None, None);
    };

    if is_audio_item(dto) {
        return (first_audio_stream_index(media_streams), None);
    }

    if !is_video_item(dto) {
        return (None, None);
    }

    let default_audio_stream_index = remembered_user_data
        .and_then(|data| {
            defaults
                .remembered_selections
                .audio
                .then_some(data.audio_stream_index)
                .flatten()
        })
        .filter(|index| is_valid_audio_stream_index(media_streams, *index))
        .or_else(|| default_audio_stream_index(media_streams, defaults, original_language));
    let audio_language = default_audio_stream_index
        .and_then(|index| {
            media_streams.iter().find(|stream| {
                stream.stream_type == MediaStreamType::Audio && stream.index == index
            })
        })
        .and_then(|stream| stream.language.clone());
    if let Some(index) = remembered_user_data
        .and_then(|data| {
            (defaults.remembered_selections.subtitle
                && defaults.subtitle_mode != SubtitlePlaybackMode::None)
                .then_some(data.subtitle_stream_index)
                .flatten()
        })
        .filter(|index| is_valid_subtitle_stream_index(media_streams, *index))
    {
        return (default_audio_stream_index, Some(index));
    }

    let default_subtitle_stream_index = MediaStreamSelector::default_subtitle_stream_index(
        media_streams,
        &defaults.subtitle_languages,
        defaults.subtitle_mode,
        audio_language.as_deref(),
    );
    MediaStreamSelector::set_subtitle_stream_scores(
        media_streams,
        &defaults.subtitle_languages,
        defaults.subtitle_mode,
        audio_language.as_deref(),
    );

    (default_audio_stream_index, default_subtitle_stream_index)
}

fn default_audio_stream_index(
    media_streams: &[MediaStream],
    defaults: &MediaStreamDefaults,
    original_language: Option<&str>,
) -> Option<i32> {
    if defaults.audio_preference == AudioLanguagePreference::OriginalLanguage {
        let original_audio_languages = normalize_language(original_language);
        if defaults.play_default_audio_track {
            return MediaStreamSelector::default_audio_stream_index(
                media_streams,
                &original_audio_languages,
                true,
            );
        }

        if let Some(stream) = original_audio_stream(media_streams, &original_audio_languages) {
            return Some(stream.index);
        }

        if !original_audio_languages.is_empty() {
            return MediaStreamSelector::default_audio_stream_index(
                media_streams,
                &original_audio_languages,
                false,
            );
        }
    }

    let AudioLanguagePreference::Languages(audio_languages) = &defaults.audio_preference else {
        return MediaStreamSelector::default_audio_stream_index(
            media_streams,
            &[],
            defaults.play_default_audio_track,
        );
    };
    MediaStreamSelector::default_audio_stream_index(
        media_streams,
        audio_languages,
        defaults.play_default_audio_track,
    )
}

fn original_audio_stream<'a>(
    media_streams: &'a [MediaStream],
    original_audio_languages: &[String],
) -> Option<&'a MediaStream> {
    let original_audio_streams = media_streams
        .iter()
        .filter(|stream| stream.stream_type == MediaStreamType::Audio && stream.is_original);
    if original_audio_languages.is_empty() {
        return original_audio_streams.into_iter().next();
    }

    original_audio_streams.into_iter().find(|stream| {
        normalize_language(stream.language.as_deref())
            .iter()
            .any(|language| {
                original_audio_languages
                    .iter()
                    .any(|original| original.eq_ignore_ascii_case(language))
            })
    })
}

async fn preferred_user_data_for_item(
    state: &AppState,
    target_user_id: Uuid,
    item: &base_item::Model,
    fields: BaseItemDtoFields,
) -> Result<Option<user_data::Model>, ApiError> {
    if !fields.wants_media_streams() {
        return Ok(None);
    }

    Ok(state
        .user_data
        .get_preferred_for_items(target_user_id, std::slice::from_ref(item))
        .await?
        .remove(&item.id))
}

fn first_audio_stream_index(media_streams: &[MediaStream]) -> Option<i32> {
    media_streams
        .iter()
        .find(|stream| stream.stream_type == MediaStreamType::Audio)
        .map(|stream| stream.index)
}

fn is_valid_audio_stream_index(media_streams: &[MediaStream], index: i32) -> bool {
    media_streams
        .iter()
        .any(|stream| stream.stream_type == MediaStreamType::Audio && stream.index == index)
}

fn is_valid_subtitle_stream_index(media_streams: &[MediaStream], index: i32) -> bool {
    index == -1
        || media_streams
            .iter()
            .any(|stream| stream.stream_type == MediaStreamType::Subtitle && stream.index == index)
}

fn is_audio_item(dto: &BaseItemDto) -> bool {
    dto.media_type
        .as_deref()
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case("Audio"))
        || dto.item_type.eq_ignore_ascii_case("Audio")
}

fn is_video_item(dto: &BaseItemDto) -> bool {
    dto.media_type
        .as_deref()
        .is_some_and(|media_type| media_type.eq_ignore_ascii_case("Video"))
        || matches!(
            dto.item_type.as_str(),
            "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
        )
}

fn normalize_language(language: Option<&str>) -> Vec<String> {
    let Some(language) = language
        .map(str::trim)
        .filter(|language| !language.is_empty())
    else {
        return Vec::new();
    };

    if let Some(culture) = LocalizationService.find_language_info(language) {
        if culture.name.contains('-') {
            vec![culture.name]
        } else {
            culture.three_letter_iso_language_names
        }
    } else {
        vec![language.to_owned()]
    }
}

pub(crate) fn original_language_from_item(item: &base_item::Model) -> Option<String> {
    metadata_string(
        item.data.as_ref(),
        &["OriginalLanguage", "original_language", "originalLanguage"],
    )
}

fn is_media_source_item(dto: &BaseItemDto) -> bool {
    dto.path.is_some()
        || dto.media_type.is_some()
        || matches!(
            dto.item_type.as_str(),
            "Audio" | "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
        )
}

fn media_container_from_path(path: &str) -> Option<String> {
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| extension.to_owned())
}

pub(crate) fn music_genre_to_dto(genre: &MusicGenre, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(genre.name.clone()),
        server_id: server_id.to_owned(),
        id: genre.id.simple().to_string(),
        item_type: "MusicGenre".to_owned(),
        etag: genre.id.simple().to_string(),
        date_created: None,
        sort_name: Some(genre.name.clone()),
        path: None,
        overview: None,
        media_type: None,
        collection_type: None,
        is_folder: true,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("MusicGenre-{}", genre.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        media_sources: None,
        media_streams: None,
    }
}

pub(crate) fn genre_to_dto(genre: &Genre, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(genre.name.clone()),
        server_id: server_id.to_owned(),
        id: genre.id.simple().to_string(),
        item_type: "Genre".to_owned(),
        etag: genre.id.simple().to_string(),
        date_created: None,
        sort_name: Some(genre.name.clone()),
        path: None,
        overview: None,
        media_type: None,
        collection_type: None,
        is_folder: true,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Genre-{}", genre.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        media_sources: None,
        media_streams: None,
    }
}

pub(crate) fn person_to_dto(person: &Person, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(person.model.name.clone()),
        server_id: server_id.to_owned(),
        id: person.model.id.simple().to_string(),
        item_type: "Person".to_owned(),
        etag: person.model.row_version.to_string(),
        date_created: Some(person.model.date_created.to_rfc3339()),
        sort_name: Some(person.model.name.clone()),
        path: None,
        overview: None,
        media_type: None,
        collection_type: None,
        is_folder: false,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Person-{}", person.model.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: Some(person.model.provider_ids.clone()),
        media_sources: None,
        media_streams: None,
    }
}

pub(crate) fn year_to_dto(year: &Year, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(year.name.clone()),
        server_id: server_id.to_owned(),
        id: year.id.simple().to_string(),
        item_type: "Year".to_owned(),
        etag: year.id.simple().to_string(),
        date_created: None,
        sort_name: Some(year.name.clone()),
        path: None,
        overview: None,
        media_type: None,
        collection_type: None,
        is_folder: true,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Year-{}", year.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        media_sources: None,
        media_streams: None,
    }
}

fn metadata_string(data: Option<&Value>, keys: &[&str]) -> Option<String> {
    let object = data?.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn metadata_value(data: Option<&Value>, keys: &[&str]) -> Option<Value> {
    let object = data?.as_object()?;
    keys.iter().find_map(|key| object.get(*key)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original_language_defaults() -> MediaStreamDefaults {
        MediaStreamDefaults {
            audio_preference: AudioLanguagePreference::OriginalLanguage,
            subtitle_languages: Vec::new(),
            play_default_audio_track: false,
            subtitle_mode: SubtitlePlaybackMode::None,
            remembered_selections: RememberedStreamSelections {
                audio: false,
                subtitle: false,
            },
        }
    }

    #[test]
    fn original_language_audio_preference_scans_past_mismatched_original_tracks() {
        let streams = [
            MediaStream {
                index: 1,
                stream_type: MediaStreamType::Audio,
                language: Some("eng".to_owned()),
                is_original: true,
                ..MediaStream::default()
            },
            MediaStream {
                index: 2,
                stream_type: MediaStreamType::Audio,
                language: Some("fre".to_owned()),
                is_original: true,
                ..MediaStream::default()
            },
        ];

        assert_eq!(
            default_audio_stream_index(&streams, &original_language_defaults(), Some("French")),
            Some(2)
        );
    }

    #[test]
    fn original_language_audio_preference_uses_first_original_track_without_item_language() {
        let streams = [
            MediaStream {
                index: 1,
                stream_type: MediaStreamType::Audio,
                language: Some("eng".to_owned()),
                is_original: true,
                ..MediaStream::default()
            },
            MediaStream {
                index: 2,
                stream_type: MediaStreamType::Audio,
                language: Some("fre".to_owned()),
                is_original: true,
                ..MediaStream::default()
            },
        ];

        assert_eq!(
            default_audio_stream_index(&streams, &original_language_defaults(), None),
            Some(1)
        );
    }
}
