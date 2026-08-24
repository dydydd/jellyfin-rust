use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use jellyfin_controller::{
    Artist, Genre, GenreKind, LocalizationService, LyricManager, MusicGenre, Person,
    RelatedItemKind, Studio, TrickplayManifest, Year, library::get_media_source_name,
};
use jellyfin_data::entities::{base_item, item_value, user_data};
use jellyfin_model::{
    MediaAttachment, MediaProtocol, MediaSourceInfo, MediaSourceType, MediaStream, MediaStreamType,
    SubtitlePlaybackMode, UserConfiguration, UserItemDataDto,
};
use jellyfin_server_implementations::{DtoImageOptions, MediaStreamSelector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserIdQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub(crate) user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UploadLyricsQuery {
    #[serde(default, rename = "fileName", alias = "FileName")]
    file_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BaseItemDtoFields {
    media_sources: bool,
    media_streams: bool,
    trickplay: bool,
}

impl BaseItemDtoFields {
    #[must_use]
    pub(crate) const fn media_sources() -> Self {
        Self {
            media_sources: true,
            media_streams: false,
            trickplay: false,
        }
    }

    #[must_use]
    pub(crate) fn from_names(fields: &[String]) -> Self {
        let mut result = Self::default();
        for field in fields {
            if field.eq_ignore_ascii_case("MediaSources") {
                result.media_sources = true;
            } else if field.eq_ignore_ascii_case("MediaStreams") {
                result.media_streams = true;
            } else if field.eq_ignore_ascii_case("Trickplay") {
                result.trickplay = true;
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

    #[must_use]
    pub(crate) const fn wants_trickplay(self) -> bool {
        self.trickplay
    }

    #[must_use]
    pub(crate) const fn without_trickplay(mut self) -> Self {
        self.trickplay = false;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[derive(Default)]
pub struct BaseItemDto {
    pub name: Option<String>,
    pub server_id: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_item_id: Option<String>,
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
    pub premiere_date: Option<String>,
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
    pub user_data: Option<UserItemDataDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<BaseItemPerson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub studios: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critic_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_metadata_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_metadata_country_code: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub production_locations: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remote_trailers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub air_days: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_subtitles: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_3d_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_locked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number_end: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airs_after_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airs_before_season_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub airs_before_episode_number: Option<i32>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub image_tags: HashMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub backdrop_image_tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_primary_image_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_backdrop_image_item_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parent_backdrop_image_tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_sources: Option<Vec<jellyfin_model::MediaSourceInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_streams: Option<Vec<jellyfin_model::MediaStream>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trickplay: Option<TrickplayManifest>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemPerson {
    pub name: String,
    pub id: String,
    pub role: String,
    #[serde(rename = "Type")]
    pub person_type: String,
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

pub(crate) async fn delete_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_lyrics() {
        return Err(ApiError::Forbidden);
    }
    state
        .user_library
        .delete_lyrics(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn upload_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UploadLyricsQuery>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_lyrics() {
        return Err(ApiError::Forbidden);
    }
    let file_name = query.file_name.as_deref().ok_or(ApiError::InvalidRequest)?;
    let format = lyric_format(file_name).ok_or(ApiError::InvalidRequest)?;
    if body.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    let content = String::from_utf8_lossy(&body);
    let lyrics = LyricManager::parse_lyrics(format, &content).ok_or(ApiError::InvalidRequest)?;
    let lyrics = state
        .user_library
        .save_lyrics(&authenticated.user, authenticated.user.id, item_id, lyrics)
        .await?;
    Ok(Json(lyrics))
}

pub(crate) async fn search_remote_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_lyrics() {
        return Err(ApiError::Forbidden);
    }
    let lyrics = state
        .user_library
        .remote_lyrics(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    Ok(Json(lyrics))
}

pub(crate) async fn download_remote_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, lyric_id)): Path<(Uuid, String)>,
) -> Result<Json<Value>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_lyrics() {
        return Err(ApiError::Forbidden);
    }
    let lyrics = state
        .user_library
        .download_remote_lyrics(
            &authenticated.user,
            authenticated.user.id,
            item_id,
            &lyric_id,
        )
        .await?;
    Ok(Json(lyrics))
}

pub(crate) async fn get_remote_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(lyric_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_lyrics() {
        return Err(ApiError::Forbidden);
    }
    let lyrics = state.user_library.get_remote_lyrics(&lyric_id)?;
    Ok(Json(lyrics))
}

fn lyric_format(file_name: &str) -> Option<&str> {
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty() && !extension.contains('/') && !extension.contains('\\'))
        .then_some(extension)
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
            target_user_id,
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
            target_user_id,
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
    let server_id = state.server_id().to_owned();
    let items = related_items(state, headers, requested_user_id, item_id, kind).await?;
    let items = items
        .into_iter()
        .map(|item| item_to_dto(item, &server_id))
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
    let server_id = state.server_id().to_owned();
    let items = related_items(state, headers, requested_user_id, item_id, kind).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|item| item_to_dto(item, &server_id))
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

#[allow(clippy::too_many_lines)]
pub(crate) fn item_to_dto(item: base_item::Model, server_id: &str) -> BaseItemDto {
    let is_user_view = item.item_type == "UserView";
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
        playlist_item_id: None,
        item_type: item.item_type,
        etag: item.row_version.to_string(),
        date_created: Some(item.date_created.to_rfc3339()),
        sort_name: item.sort_name,
        path: item.path,
        overview: item.overview,
        media_type: item.media_type,
        collection_type: if is_user_view {
            metadata_string(
                item.data.as_ref(),
                &["ViewType", "view_type", "CollectionType", "collection_type"],
            )
        } else {
            None
        },
        is_folder: item.is_folder,
        is_virtual_item: item.is_virtual_item,
        parent_id: item.parent_id.map(|id| id.simple().to_string()),
        index_number: item.index_number,
        parent_index_number: item.parent_index_number,
        production_year: item.production_year,
        premiere_date: item.premiere_date.map(|date| date.to_rfc3339()),
        run_time_ticks: item.runtime_ticks,
        presentation_unique_key: item.presentation_unique_key,
        series_id: item.series_id.map(|id| id.simple().to_string()),
        season_id: item.season_id.map(|id| id.simple().to_string()),
        extra_type,
        has_lyrics,
        provider_ids: metadata_value(item.data.as_ref(), &["ProviderIds", "provider_ids"]),
        user_data: None,
        genres: metadata_strings(item.data.as_ref(), &["Genres", "genres"]),
        people: Vec::new(),
        tags: metadata_strings(item.data.as_ref(), &["Tags", "tags"]),
        studios: metadata_strings(item.data.as_ref(), &["Studios", "studios"]),
        community_rating: metadata_f64(
            item.data.as_ref(),
            &["CommunityRating", "community_rating"],
        ),
        critic_rating: metadata_f64(item.data.as_ref(), &["CriticRating", "critic_rating"]),
        official_rating: item.official_rating,
        original_title: metadata_string(
            item.data.as_ref(),
            &["OriginalTitle", "original_title", "originalTitle"],
        ),
        tagline: metadata_string(item.data.as_ref(), &["Tagline", "tagline"]),
        status: metadata_string(item.data.as_ref(), &["Status", "status"]),
        custom_rating: metadata_string(item.data.as_ref(), &["CustomRating", "custom_rating"]),
        collection_name: metadata_string(
            item.data.as_ref(),
            &["CollectionName", "collection_name"],
        ),
        aspect_ratio: metadata_string(item.data.as_ref(), &["AspectRatio", "aspect_ratio"]),
        preferred_metadata_language: metadata_string(
            item.data.as_ref(),
            &["PreferredMetadataLanguage", "preferred_metadata_language"],
        ),
        preferred_metadata_country_code: metadata_string(
            item.data.as_ref(),
            &[
                "PreferredMetadataCountryCode",
                "preferred_metadata_country_code",
            ],
        ),
        production_locations: metadata_strings(
            item.data.as_ref(),
            &["ProductionLocations", "production_locations"],
        ),
        remote_trailers: metadata_strings(
            item.data.as_ref(),
            &["RemoteTrailers", "remote_trailers"],
        ),
        air_days: metadata_strings(item.data.as_ref(), &["AirDays", "air_days"]),
        end_date: metadata_string(item.data.as_ref(), &["EndDate", "end_date"]),
        width: metadata_i32(item.data.as_ref(), &["Width", "width"]),
        height: metadata_i32(item.data.as_ref(), &["Height", "height"]),
        has_subtitles: metadata_bool(item.data.as_ref(), &["HasSubtitles", "has_subtitles"]),
        video_3d_format: metadata_string(item.data.as_ref(), &["Video3DFormat", "video_3d_format"]),
        is_locked: metadata_bool(item.data.as_ref(), &["IsLocked", "is_locked"]),
        index_number_end: metadata_i32(item.data.as_ref(), &["IndexNumberEnd", "index_number_end"]),
        airs_after_season_number: metadata_i32(
            item.data.as_ref(),
            &["AirsAfterSeasonNumber", "airs_after_season_number"],
        ),
        airs_before_season_number: metadata_i32(
            item.data.as_ref(),
            &["AirsBeforeSeasonNumber", "airs_before_season_number"],
        ),
        airs_before_episode_number: metadata_i32(
            item.data.as_ref(),
            &["AirsBeforeEpisodeNumber", "airs_before_episode_number"],
        ),
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
    }
}

pub(crate) async fn project_item_to_dto(
    state: &AppState,
    item: base_item::Model,
    target_user_id: Uuid,
    fields: BaseItemDtoFields,
    defaults: Option<&MediaStreamDefaults>,
    remembered_user_data: Option<&user_data::Model>,
) -> Result<BaseItemDto, ApiError> {
    let item_id = item.id;
    let original_language = original_language_from_item(&item);
    let mut relations = load_relation_metadata(state, std::slice::from_ref(&item)).await?;
    let user_data = user_data_for_item(state, &item, target_user_id).await?;
    let mut dto = item_to_dto(item, state.server_id());
    attach_relation_metadata(&mut dto, relations.remove(&item_id).unwrap_or_default());
    attach_user_data_dto(&mut dto, user_data);
    if let Some(projection) = state
        .dto_images
        .project(item_id, DtoImageOptions::default())
        .await
        .map_err(|_| ApiError::Internal)?
    {
        attach_dto_image_projection(&mut dto, projection);
    }
    if fields.wants_trickplay() && is_video_item(&dto) {
        dto.trickplay = Some(
            state
                .trickplay
                .manifests_for_items(&[item_id])
                .await?
                .remove(&item_id)
                .unwrap_or_default(),
        );
    }
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

pub(crate) fn attach_dto_image_projection(
    dto: &mut BaseItemDto,
    projection: jellyfin_server_implementations::DtoImageProjection,
) {
    dto.image_tags = projection.image_tags;
    dto.backdrop_image_tags = projection.backdrop_image_tags;
    dto.parent_primary_image_item_id = projection
        .parent_primary_image_item_id
        .map(|id| id.simple().to_string());
    dto.parent_primary_image_tag = projection.parent_primary_image_tag;
    dto.primary_image_aspect_ratio = projection.primary_image_aspect_ratio;
    dto.series_primary_image_tag = projection.series_primary_image_tag;
    dto.parent_backdrop_image_item_id = projection
        .parent_backdrop_image_item_id
        .map(|id| id.simple().to_string());
    dto.parent_backdrop_image_tags = projection.parent_backdrop_image_tags;
}

#[derive(Debug, Default)]
pub(crate) struct ItemRelationMetadata {
    genres: Vec<String>,
    people: Vec<BaseItemPerson>,
    tags: Vec<String>,
    studios: Vec<String>,
}

pub(crate) async fn load_relation_metadata(
    state: &AppState,
    items: &[base_item::Model],
) -> Result<HashMap<Uuid, ItemRelationMetadata>, ApiError> {
    let item_ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    let genres = state
        .item_values
        .values_for_items(&item_ids, item_value::ItemValueType::Genre)
        .await
        .map_err(|_| ApiError::Internal)?;
    let tags = state
        .item_values
        .values_for_items(&item_ids, item_value::ItemValueType::Tags)
        .await
        .map_err(|_| ApiError::Internal)?;
    let studios = state
        .item_values
        .values_for_items(&item_ids, item_value::ItemValueType::Studios)
        .await
        .map_err(|_| ApiError::Internal)?;
    let people = state
        .people
        .people_for_items(&item_ids)
        .await
        .map_err(|_| ApiError::Internal)?;

    let mut result = HashMap::with_capacity(items.len());
    for item in items {
        let mut metadata = ItemRelationMetadata::default();
        if let Some(values) = genres.get(&item.id) {
            metadata.genres.clone_from(values);
        }
        if let Some(values) = tags.get(&item.id) {
            metadata.tags.clone_from(values);
        }
        if let Some(values) = studios.get(&item.id) {
            metadata.studios.clone_from(values);
        }
        if let Some(credits) = people.get(&item.id) {
            metadata.people = credits
                .iter()
                .map(|credit| BaseItemPerson {
                    name: credit.person.name.clone(),
                    id: credit.person.id.simple().to_string(),
                    role: credit.role.clone(),
                    person_type: credit.person_type.clone(),
                })
                .collect();
        }
        result.insert(item.id, metadata);
    }
    Ok(result)
}

pub(crate) fn attach_relation_metadata(dto: &mut BaseItemDto, metadata: ItemRelationMetadata) {
    if !metadata.genres.is_empty() {
        dto.genres = metadata.genres;
    }
    dto.people = metadata.people;
    if !metadata.tags.is_empty() {
        dto.tags = metadata.tags;
    }
    if !metadata.studios.is_empty() {
        dto.studios = metadata.studios;
    }
}

pub(crate) fn attach_user_data_dto(dto: &mut BaseItemDto, user_data: UserItemDataDto) {
    dto.user_data = Some(user_data);
}

async fn user_data_for_item(
    state: &AppState,
    item: &base_item::Model,
    target_user_id: Uuid,
) -> Result<UserItemDataDto, ApiError> {
    let user_data = state
        .user_data
        .preferred_dto_map(target_user_id, std::slice::from_ref(item))
        .await?
        .remove(&item.id)
        .unwrap_or_else(|| UserItemDataDto {
            rating: None,
            played_percentage: None,
            unplayed_item_count: None,
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            likes: None,
            last_played_date: None,
            played: false,
            key: item.id.simple().to_string(),
            item_id: item.id.simple().to_string(),
        });
    Ok(user_data)
}

pub(crate) async fn trickplay_manifests_for_items(
    state: &AppState,
    items: &[base_item::Model],
    fields: BaseItemDtoFields,
) -> Result<jellyfin_controller::TrickplayManifests, ApiError> {
    if !fields.wants_trickplay() {
        return Ok(HashMap::default());
    }
    let item_ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    Ok(state.trickplay.manifests_for_items(&item_ids).await?)
}

pub(crate) fn attach_trickplay_manifest(
    dto: &mut BaseItemDto,
    fields: BaseItemDtoFields,
    manifest: TrickplayManifest,
) {
    if fields.wants_trickplay() && is_video_item(dto) {
        dto.trickplay = Some(manifest);
    }
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
        playlist_item_id: None,
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
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("MusicGenre-{}", genre.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn genre_to_dto(genre: &Genre, server_id: &str) -> BaseItemDto {
    let (item_type, presentation_prefix) = match genre.kind {
        GenreKind::Genre => ("Genre", "Genre"),
        GenreKind::MusicGenre => ("MusicGenre", "MusicGenre"),
    };
    BaseItemDto {
        name: Some(genre.name.clone()),
        server_id: server_id.to_owned(),
        id: genre.id.simple().to_string(),
        playlist_item_id: None,
        item_type: item_type.to_owned(),
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
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("{presentation_prefix}-{}", genre.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn studio_to_dto(studio: &Studio, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(studio.name.clone()),
        server_id: server_id.to_owned(),
        id: studio.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "Studio".to_owned(),
        etag: studio.id.simple().to_string(),
        date_created: None,
        sort_name: Some(studio.name.clone()),
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
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Studio-{}", studio.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn artist_to_dto(artist: &Artist, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(artist.name.clone()),
        server_id: server_id.to_owned(),
        id: artist.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "MusicArtist".to_owned(),
        etag: artist.id.simple().to_string(),
        date_created: None,
        sort_name: Some(artist.name.clone()),
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
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Artist-{}", artist.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn person_to_dto(person: &Person, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(person.model.name.clone()),
        server_id: server_id.to_owned(),
        id: person.model.id.simple().to_string(),
        playlist_item_id: None,
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
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Person-{}", person.model.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: Some(person.model.provider_ids.clone()),
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn year_to_dto(year: &Year, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(year.name.clone()),
        server_id: server_id.to_owned(),
        id: year.id.simple().to_string(),
        playlist_item_id: None,
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
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Year-{}", year.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

fn metadata_value(data: Option<&Value>, keys: &[&str]) -> Option<Value> {
    let object = data?.as_object()?;
    keys.iter().find_map(|key| object.get(*key)).cloned()
}

fn metadata_string(data: Option<&Value>, keys: &[&str]) -> Option<String> {
    metadata_value(data, keys).and_then(|value| value.as_str().map(str::to_owned))
}

fn metadata_f64(data: Option<&Value>, keys: &[&str]) -> Option<f64> {
    metadata_value(data, keys).and_then(|value| value.as_f64())
}

fn metadata_i32(data: Option<&Value>, keys: &[&str]) -> Option<i32> {
    metadata_value(data, keys)
        .and_then(|value| value.as_i64().and_then(|value| i32::try_from(value).ok()))
}

fn metadata_bool(data: Option<&Value>, keys: &[&str]) -> Option<bool> {
    metadata_value(data, keys).and_then(|value| value.as_bool())
}

fn metadata_strings(data: Option<&Value>, keys: &[&str]) -> Vec<String> {
    metadata_value(data, keys)
        .and_then(|value| value.as_array().cloned())
        .map(|values| {
            values
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn item_to_dto_projects_persisted_metadata_json() {
        let item = base_item::Model {
            id: Uuid::new_v4(),
            item_type: "Movie".to_owned(),
            data: Some(json!({
                "CommunityRating": 8.5,
                "CriticRating": 7.0,
                "OriginalTitle": "Original",
                "Tagline": "Tag",
                "Status": "Ended",
                "IsLocked": true,
                "Width": 1920,
                "Height": 1080,
                "AirDays": ["Monday", "Friday"],
                "ProductionLocations": ["Los Angeles"]
            })),
            path: None,
            parent_id: None,
            top_parent_id: None,
            name: Some("Movie".to_owned()),
            clean_name: None,
            sort_name: None,
            media_type: None,
            overview: None,
            official_rating: Some("PG-13".to_owned()),
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: chrono::DateTime::UNIX_EPOCH,
            date_modified: chrono::DateTime::UNIX_EPOCH,
            row_version: 1,
        };

        let dto = item_to_dto(item, "server");

        assert_eq!(dto.community_rating, Some(8.5));
        assert_eq!(dto.critic_rating, Some(7.0));
        assert_eq!(dto.original_title.as_deref(), Some("Original"));
        assert_eq!(dto.tagline.as_deref(), Some("Tag"));
        assert_eq!(dto.status.as_deref(), Some("Ended"));
        assert_eq!(dto.is_locked, Some(true));
        assert_eq!(dto.width, Some(1920));
        assert_eq!(dto.height, Some(1080));
        assert_eq!(dto.air_days, ["Monday", "Friday"]);
        assert_eq!(dto.production_locations, ["Los Angeles"]);
        assert_eq!(dto.official_rating.as_deref(), Some("PG-13"));
    }

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
