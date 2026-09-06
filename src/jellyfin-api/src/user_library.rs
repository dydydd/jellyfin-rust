use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use jellyfin_controller::{
    Artist, Genre, GenreKind, LocalizationService, LyricManager, MusicGenre, Person,
    RelatedItemKind, Studio, TrickplayManifest, Year, decode_lyric_bytes,
    library::{get_common_media_source_prefix, get_media_source_name},
};
use jellyfin_data::{
    ItemValueCounts,
    entities::{base_item, item_value, user_data},
};
use jellyfin_model::{
    IsoType, MediaAttachment, MediaProtocol, MediaSourceInfo, MediaSourceType, MediaStream,
    MediaStreamType, MediaUrl, NameIdPair, PersonKind, SubtitlePlaybackMode,
    TransportStreamTimestamp, UserConfiguration, UserItemDataDto, Video3DFormat, VideoType,
};
use jellyfin_server_implementations::{DtoImageOptions, MediaStreamSelector};
use md5::{Digest, Md5};
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
    #[serde(default, rename = "fileName", alias = "FileName", alias = "filename")]
    file_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BaseItemDtoFields {
    media_sources: bool,
    media_streams: bool,
    media_source_count: bool,
    item_counts: bool,
    child_count: bool,
    recursive_item_count: bool,
    primary_image_aspect_ratio: bool,
    trickplay: bool,
}

impl BaseItemDtoFields {
    #[must_use]
    pub(crate) const fn all() -> Self {
        Self {
            media_sources: true,
            media_streams: true,
            media_source_count: true,
            item_counts: true,
            child_count: true,
            recursive_item_count: true,
            primary_image_aspect_ratio: true,
            trickplay: true,
        }
    }

    #[must_use]
    pub(crate) const fn media_sources() -> Self {
        Self {
            media_sources: true,
            media_streams: false,
            media_source_count: false,
            item_counts: false,
            child_count: false,
            recursive_item_count: false,
            primary_image_aspect_ratio: false,
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
            } else if field.eq_ignore_ascii_case("MediaSourceCount") {
                result.media_source_count = true;
            } else if field.eq_ignore_ascii_case("ItemCounts") {
                result.item_counts = true;
            } else if field.eq_ignore_ascii_case("ChildCount") {
                result.child_count = true;
            } else if field.eq_ignore_ascii_case("RecursiveItemCount") {
                result.recursive_item_count = true;
            } else if field.eq_ignore_ascii_case("PrimaryImageAspectRatio") {
                result.primary_image_aspect_ratio = true;
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
    pub(crate) const fn wants_media_sources(self) -> bool {
        self.media_sources
    }

    #[must_use]
    pub(crate) const fn wants_media_attachments(self) -> bool {
        self.media_sources
    }

    #[must_use]
    pub(crate) const fn wants_media_source_count(self) -> bool {
        self.media_source_count
    }

    #[must_use]
    pub(crate) const fn wants_item_counts(self) -> bool {
        self.item_counts
    }

    #[must_use]
    pub(crate) const fn wants_child_count(self) -> bool {
        self.child_count
    }

    #[must_use]
    pub(crate) const fn wants_recursive_item_count(self) -> bool {
        self.recursive_item_count
    }

    #[must_use]
    pub(crate) const fn wants_primary_image_aspect_ratio(self) -> bool {
        self.primary_image_aspect_ratio
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
    #[serde(skip)]
    pub(crate) media_source_path: Option<String>,
    #[serde(skip)]
    pub(crate) media_source_bitrate: Option<i32>,
    #[serde(skip)]
    pub(crate) media_source_container: Option<String>,
    #[serde(skip)]
    pub(crate) media_source_size: Option<i64>,
    #[serde(skip)]
    pub(crate) media_source_etag: Option<String>,
    #[serde(skip)]
    pub(crate) media_source_timestamp: Option<TransportStreamTimestamp>,
    #[serde(skip)]
    pub(crate) album_artist_names: Vec<String>,
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
    pub child_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recursive_item_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_video_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trailer_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_preferences_id: Option<String>,
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
    pub media_source_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_unique_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artists: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_items: Option<Vec<NameIdPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artists: Option<Vec<NameIdPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_lyrics: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<UserItemDataDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genre_items: Vec<NameIdPair>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<BaseItemPerson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub studios: Vec<NameIdPair>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub critic_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub taglines: Vec<String>,
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
    pub remote_trailers: Vec<MediaUrl>,
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
    #[serde(rename = "Video3DFormat")]
    pub video_3d_format: Option<Video3DFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso_type: Option<IsoType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "LockData")]
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
    #[serde(rename = "ParentBackdropItemId")]
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
    pub person_type: PersonKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
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
) -> Result<Json<BaseItemDto>, ApiError> {
    get_item_for(
        state,
        headers,
        Some(user_id),
        item_id,
        BaseItemDtoFields::all(),
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
        BaseItemDtoFields::all(),
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
    let content = decode_lyric_bytes(&body);
    let lyrics = LyricManager::parse_lyrics(format, &content).ok_or(ApiError::InvalidRequest)?;
    let lyrics = state
        .user_library
        .save_lyrics(
            &authenticated.user,
            authenticated.user.id,
            item_id,
            format,
            body.as_ref(),
            lyrics,
        )
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
    if file_name.is_empty()
        || file_name.trim() != file_name
        || file_name.contains(['/', '\\', '\0'])
        || file_name.chars().any(char::is_control)
    {
        return None;
    }
    let (stem, extension) = file_name.rsplit_once('.')?;
    (!stem.is_empty()
        && stem != "."
        && stem != ".."
        && !extension.is_empty()
        && extension != "."
        && extension != "..")
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
    let mut child_counts = child_counts_for_items(
        state.as_ref(),
        std::slice::from_ref(&item),
        requested_fields,
        target_user_id,
    )
    .await?;
    let mut recursive_item_counts = recursive_item_counts_for_items(
        state.as_ref(),
        std::slice::from_ref(&item),
        requested_fields,
        target_user_id,
    )
    .await?;
    let item_id = item.id;
    let mut dto = project_item_to_dto(
        state.as_ref(),
        item,
        target_user_id,
        requested_fields,
        defaults.as_ref(),
        remembered_user_data.as_ref(),
    )
    .await?;
    attach_child_count(&mut dto, child_counts.remove(&item_id));
    attach_recursive_item_count(&mut dto, recursive_item_counts.remove(&item_id));
    Ok(Json(dto))
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
    let mut child_counts = child_counts_for_items(
        state.as_ref(),
        std::slice::from_ref(&item),
        requested_fields,
        target_user_id,
    )
    .await?;
    let mut recursive_item_counts = recursive_item_counts_for_items(
        state.as_ref(),
        std::slice::from_ref(&item),
        requested_fields,
        target_user_id,
    )
    .await?;
    let item_id = item.id;
    let mut dto = project_item_to_dto(
        state.as_ref(),
        item,
        target_user_id,
        requested_fields,
        defaults.as_ref(),
        remembered_user_data.as_ref(),
    )
    .await?;
    attach_child_count(&mut dto, child_counts.remove(&item_id));
    attach_recursive_item_count(&mut dto, recursive_item_counts.remove(&item_id));
    Ok(Json(dto))
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
    let has_artists = has_artist_fields(&item.item_type);
    let has_album_artists = has_album_artist_fields(&item.item_type);
    let has_album =
        is_item_type(&item.item_type, "Audio") || is_item_type(&item.item_type, "MusicVideo");
    let artists =
        has_artists.then(|| metadata_strings(item.data.as_ref(), &["Artists", "artists"]));
    let album_artist_names = has_album_artists.then(|| {
        let mut names = metadata_strings(
            item.data.as_ref(),
            &["AlbumArtists", "albumArtists", "album_artists"],
        );
        if names.is_empty()
            && let Some(name) = metadata_string(
                item.data.as_ref(),
                &["AlbumArtist", "albumArtist", "album_artist"],
            )
        {
            names.push(name);
        }
        names
    });
    let album_artist = album_artist_names
        .as_ref()
        .and_then(|names| names.first().cloned());
    let extra_type = metadata_string(item.data.as_ref(), &["ExtraType", "extra_type"])
        .map(|value| canonical_enum_or(&value, EXTRA_TYPES, "Unknown"));
    let media_source_path = metadata_string(item.data.as_ref(), &["StrmTarget", "strm_target"]);
    let media_source_bitrate = metadata_i32(item.data.as_ref(), &["Bitrate", "bitrate"]);
    let media_source_container = metadata_string(item.data.as_ref(), &["Container", "container"]);
    let media_source_size = metadata_i64(item.data.as_ref(), &["Size", "size"]);
    let media_source_etag = media_source_etag(item.date_modified);
    let media_source_timestamp = metadata_enum(
        item.data.as_ref(),
        &["Timestamp", "timestamp"],
        TRANSPORT_STREAM_TIMESTAMPS,
    );
    let video_3d_format = metadata_enum(
        item.data.as_ref(),
        &["Video3DFormat", "video3DFormat", "video_3d_format"],
        VIDEO_3D_FORMATS,
    );
    let iso_type = metadata_enum(
        item.data.as_ref(),
        &["IsoType", "isoType", "iso_type"],
        ISO_TYPES,
    );
    let has_lyrics = item
        .data
        .as_ref()
        .and_then(Value::as_object)
        .map(|object| object.contains_key("Lyrics") || object.contains_key("lyrics"));
    let original_language = original_language_from_item(&item);
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
        media_source_path,
        media_source_bitrate,
        media_source_container,
        media_source_size,
        media_source_etag,
        media_source_timestamp,
        album_artist_names: album_artist_names.clone().unwrap_or_default(),
        overview: item.overview,
        media_type: item
            .media_type
            .as_deref()
            .map(|value| canonical_enum_or(value, MEDIA_TYPES, "Unknown")),
        collection_type: if is_user_view {
            metadata_string(
                item.data.as_ref(),
                &["ViewType", "view_type", "CollectionType", "collection_type"],
            )
            .map(|value| canonical_enum_or(&value, COLLECTION_TYPES, "unknown"))
        } else {
            None
        },
        is_folder: item.is_folder,
        is_virtual_item: item.is_virtual_item,
        parent_id: item.parent_id.map(|id| id.simple().to_string()),
        child_count: None,
        recursive_item_count: None,
        album_count: None,
        artist_count: None,
        episode_count: None,
        movie_count: None,
        music_video_count: None,
        program_count: None,
        series_count: None,
        song_count: None,
        trailer_count: None,
        display_preferences_id: None,
        index_number: item.index_number,
        parent_index_number: item.parent_index_number,
        production_year: item.production_year,
        premiere_date: item.premiere_date.map(|date| date.to_rfc3339()),
        run_time_ticks: item.runtime_ticks,
        media_source_count: None,
        presentation_unique_key: item.presentation_unique_key,
        series_id: item.series_id.map(|id| id.simple().to_string()),
        series_name: metadata_string(item.data.as_ref(), &["SeriesName", "series_name"]),
        season_id: item.season_id.map(|id| id.simple().to_string()),
        season_name: metadata_string(item.data.as_ref(), &["SeasonName", "season_name"]),
        album: has_album
            .then(|| metadata_string(item.data.as_ref(), &["Album", "album"]))
            .flatten(),
        album_id: None,
        artists,
        artist_items: has_artists.then(Vec::new),
        album_artist,
        album_artists: album_artist_names.map(|_| Vec::new()),
        extra_type,
        has_lyrics,
        provider_ids: metadata_provider_ids(item.data.as_ref()),
        user_data: None,
        genres: metadata_strings(item.data.as_ref(), &["Genres", "genres"]),
        genre_items: Vec::new(),
        people: Vec::new(),
        tags: metadata_strings(item.data.as_ref(), &["Tags", "tags"]),
        studios: Vec::new(),
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
        original_language,
        taglines: metadata_taglines(item.data.as_ref()),
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
        remote_trailers: metadata_remote_trailers(item.data.as_ref()),
        air_days: metadata_enum_strings(item.data.as_ref(), &["AirDays", "air_days"], AIR_DAYS),
        end_date: metadata_api_datetime(item.data.as_ref(), &["EndDate", "end_date"]),
        width: metadata_i32(item.data.as_ref(), &["Width", "width"]),
        height: metadata_i32(item.data.as_ref(), &["Height", "height"]),
        has_subtitles: metadata_bool(item.data.as_ref(), &["HasSubtitles", "has_subtitles"]),
        video_3d_format,
        iso_type,
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
    let mut relations = load_relation_metadata(state, std::slice::from_ref(&item)).await?;
    let user_data = user_data_for_item(state, &item, target_user_id).await?;
    let mut dto = item_to_dto(item, state.server_id());
    let original_language = dto.original_language.clone();
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
    if fields.wants_media_source_count() && !fields.media_sources {
        let count = state
            .user_library
            .visible_media_source_counts(target_user_id, &[item_id])
            .await?
            .remove(&item_id)
            .unwrap_or_default();
        attach_media_source_count(&mut dto, count);
    }
    if !fields.wants_media_streams() {
        return Ok(dto);
    }

    if fields.media_sources {
        let mut source_items = state.base_items.media_source_versions(item_id).await?;
        if !source_items.is_empty() {
            let source_ids = source_items.iter().map(|item| item.id).collect::<Vec<_>>();
            let visible_source_ids = state
                .user_library
                .visible_item_ids(target_user_id, &source_ids)
                .await?;
            // The official source manager always retains the explicitly displayed source and
            // applies standalone visibility checks only to its alternate versions.
            source_items
                .retain(|source| source.id == item_id || visible_source_ids.contains(&source.id));
            if fields.wants_media_source_count() {
                attach_media_source_count(
                    &mut dto,
                    u64::try_from(source_items.len()).unwrap_or(u64::MAX),
                );
            }
            attach_versioned_media_sources(
                state,
                &mut dto,
                source_items,
                fields,
                defaults,
                remembered_user_data,
            )
            .await?;
            return Ok(dto);
        }
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

pub(crate) fn attach_media_source_count(dto: &mut BaseItemDto, count: u64) {
    dto.media_source_count = (count > 1).then(|| i32::try_from(count).ok()).flatten();
}

async fn attach_versioned_media_sources(
    state: &AppState,
    dto: &mut BaseItemDto,
    source_items: Vec<base_item::Model>,
    fields: BaseItemDtoFields,
    defaults: Option<&MediaStreamDefaults>,
    remembered_user_data: Option<&user_data::Model>,
) -> Result<(), ApiError> {
    let source_ids = source_items.iter().map(|item| item.id).collect::<Vec<_>>();
    let mut media_streams = state
        .media_streams
        .get_media_streams_for_items(&source_ids)
        .await?;
    let mut media_attachments = state
        .media_attachments
        .get_media_attachments_for_items(&source_ids)
        .await?;
    let linked_alternate_version_parents = state
        .base_items
        .linked_alternate_version_parents(&source_ids)
        .await?;
    project_item_dto_with_versioned_sources(
        dto,
        source_items,
        state.server_id(),
        fields,
        &mut media_streams,
        &mut media_attachments,
        defaults,
        remembered_user_data,
        &linked_alternate_version_parents,
    )
}

pub(crate) fn project_item_dto_with_versioned_sources(
    dto: &mut BaseItemDto,
    mut source_items: Vec<base_item::Model>,
    server_id: &str,
    fields: BaseItemDtoFields,
    media_streams: &mut HashMap<Uuid, Vec<MediaStream>>,
    media_attachments: &mut HashMap<Uuid, Vec<MediaAttachment>>,
    defaults: Option<&MediaStreamDefaults>,
    remembered_user_data: Option<&user_data::Model>,
    linked_alternate_version_parents: &HashMap<Uuid, Uuid>,
) -> Result<(), ApiError> {
    let requested_id = Uuid::parse_str(&dto.id).map_err(|_| ApiError::Internal)?;
    if let Some(index) = source_items.iter().position(|item| item.id == requested_id) {
        source_items.swap(0, index);
    }
    let source_paths = source_items
        .iter()
        .filter_map(|item| item.path.as_deref())
        .collect::<Vec<_>>();
    let has_local_alternates = source_paths.len() > 1;
    let common_prefix = has_local_alternates
        .then(|| get_common_media_source_prefix(&source_paths))
        .filter(|prefix| !prefix.is_empty());
    let mut sources = Vec::with_capacity(source_items.len());

    for source_item in source_items {
        let source_id = source_item.id;
        let source_dto = item_to_dto(source_item, server_id);
        let original_language = source_dto.original_language.clone();
        let mut streams = media_streams.remove(&source_id).unwrap_or_default();
        let (default_audio_stream_index, default_subtitle_stream_index) =
            apply_media_stream_defaults(
                &source_dto,
                &mut streams,
                defaults,
                remembered_user_data,
                original_language.as_deref(),
            );
        if source_id == requested_id && fields.media_streams {
            // ALLOW: the official DTO exposes the selected source streams both here and nested.
            dto.media_streams = Some(streams.clone());
        }
        if let Some(mut source) = media_source_from_dto(
            &source_dto,
            streams,
            media_attachments.remove(&source_id).unwrap_or_default(),
            default_audio_stream_index,
            default_subtitle_stream_index,
            has_local_alternates,
            common_prefix.as_deref(),
        ) {
            if source_id != requested_id
                && (linked_alternate_version_parents.contains_key(&source_id)
                    || linked_alternate_version_parents.get(&requested_id) == Some(&source_id))
            {
                source.source_type = MediaSourceType::Grouping;
            }
            sources.push(source);
        }
    }
    dto.media_sources = Some(sources);
    Ok(())
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
    genres: Vec<NameIdPair>,
    artist_items: Vec<NameIdPair>,
    album_artist_items: Vec<NameIdPair>,
    album_id: Option<Uuid>,
    people: Vec<BaseItemPerson>,
    tags: Vec<String>,
    studios: Vec<NameIdPair>,
}

#[derive(Debug, Default)]
struct MusicRelationMetadata {
    artist_items: HashMap<Uuid, Vec<NameIdPair>>,
    album_artist_items: HashMap<Uuid, Vec<NameIdPair>>,
    album_ids: HashMap<Uuid, Uuid>,
}

pub(crate) async fn load_relation_metadata(
    state: &AppState,
    items: &[base_item::Model],
) -> Result<HashMap<Uuid, ItemRelationMetadata>, ApiError> {
    let item_ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    let mut genres = state
        .item_values
        .value_pairs_for_items(&item_ids, item_value::ItemValueType::Genre)
        .await
        .map_err(|_| ApiError::Internal)?;
    let mut tags = state
        .item_values
        .values_for_items(&item_ids, item_value::ItemValueType::Tags)
        .await
        .map_err(|_| ApiError::Internal)?;
    let mut studios = state
        .item_values
        .value_pairs_for_items(&item_ids, item_value::ItemValueType::Studios)
        .await
        .map_err(|_| ApiError::Internal)?;
    let mut music = load_music_relation_metadata(state, items).await?;
    let mut people = state
        .people
        .people_for_items(&item_ids)
        .await
        .map_err(|_| ApiError::Internal)?;
    let person_ids = people
        .values()
        .flatten()
        .map(|credit| credit.person.id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let person_image_tags = state
        .dto_images
        .primary_image_tags(&person_ids)
        .await
        .map_err(|_| ApiError::Internal)?;

    let mut result = HashMap::with_capacity(items.len());
    for item in items {
        let metadata = ItemRelationMetadata {
            genres: genres
                .remove(&item.id)
                .unwrap_or_default()
                .into_iter()
                .map(|genre| NameIdPair {
                    name: genre.value,
                    id: genre.id.simple().to_string(),
                })
                .collect(),
            artist_items: music.artist_items.remove(&item.id).unwrap_or_default(),
            album_artist_items: music
                .album_artist_items
                .remove(&item.id)
                .unwrap_or_default(),
            album_id: music.album_ids.remove(&item.id),
            people: people
                .remove(&item.id)
                .unwrap_or_default()
                .into_iter()
                .map(|credit| BaseItemPerson {
                    name: credit.person.name,
                    id: credit.person.id.simple().to_string(),
                    role: credit.role,
                    person_type: person_kind_from_name(&credit.person_type),
                    primary_image_tag: person_image_tags.get(&credit.person.id).cloned(),
                })
                .collect(),
            tags: tags.remove(&item.id).unwrap_or_default(),
            studios: studios
                .remove(&item.id)
                .unwrap_or_default()
                .into_iter()
                .map(|studio| NameIdPair {
                    name: studio.value,
                    id: studio.id.simple().to_string(),
                })
                .collect(),
        };
        result.insert(item.id, metadata);
    }
    Ok(result)
}

async fn load_music_relation_metadata(
    state: &AppState,
    items: &[base_item::Model],
) -> Result<MusicRelationMetadata, ApiError> {
    let artist_item_ids = items
        .iter()
        .filter(|item| has_artist_fields(&item.item_type))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let album_artist_item_ids = items
        .iter()
        .filter(|item| has_album_artist_fields(&item.item_type))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let audio_ids = items
        .iter()
        .filter(|item| is_item_type(&item.item_type, "Audio"))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let (artist_items, album_artist_items, album_ids) = tokio::try_join!(
        async {
            state
                .item_values
                .value_pairs_for_items(&artist_item_ids, item_value::ItemValueType::Artist)
                .await
                .map_err(|_| ApiError::Internal)
        },
        async {
            state
                .item_values
                .value_pairs_for_items(
                    &album_artist_item_ids,
                    item_value::ItemValueType::AlbumArtist,
                )
                .await
                .map_err(|_| ApiError::Internal)
        },
        async {
            state
                .base_items
                .nearest_ancestor_ids_by_type(&audio_ids, &["MusicAlbum".to_owned()])
                .await
                .map_err(ApiError::from)
        },
    )?;
    Ok(MusicRelationMetadata {
        artist_items: artist_items
            .into_iter()
            .map(|(item_id, artists)| (item_id, item_value_pairs_to_dto(artists)))
            .collect(),
        album_artist_items: album_artist_items
            .into_iter()
            .map(|(item_id, artists)| (item_id, item_value_pairs_to_dto(artists)))
            .collect(),
        album_ids,
    })
}

fn item_value_pairs_to_dto(pairs: Vec<jellyfin_data::ItemValuePair>) -> Vec<NameIdPair> {
    pairs
        .into_iter()
        .map(|pair| NameIdPair {
            name: pair.value,
            id: pair.id.simple().to_string(),
        })
        .collect()
}

pub(crate) fn attach_relation_metadata(dto: &mut BaseItemDto, metadata: ItemRelationMetadata) {
    if !metadata.genres.is_empty() {
        dto.genres = metadata
            .genres
            .iter()
            .map(|genre| genre.name.clone())
            .collect();
        dto.genre_items = metadata.genres;
    }
    dto.people = metadata.people;
    if !metadata.tags.is_empty() {
        dto.tags = metadata.tags;
    }
    if !metadata.studios.is_empty() {
        dto.studios = metadata.studios;
    }
    if let Some(album_id) = metadata.album_id {
        dto.album_id = Some(album_id.simple().to_string());
    }
    if let Some(artists) = dto.artists.as_ref() {
        dto.artist_items = Some(ordered_name_id_pairs(artists, &metadata.artist_items));
    }
    if let Some(album_artists) = dto.album_artists.as_mut() {
        *album_artists =
            ordered_name_id_pairs(&dto.album_artist_names, &metadata.album_artist_items);
    }
}

fn ordered_name_id_pairs(names: &[String], pairs: &[NameIdPair]) -> Vec<NameIdPair> {
    let mut seen = std::collections::HashSet::new();
    names
        .iter()
        .filter(|name| !name.trim().is_empty())
        .filter(|name| seen.insert((*name).clone()))
        .filter_map(|name| {
            pairs
                .iter()
                .find(|pair| pair.name.eq_ignore_ascii_case(name))
                .map(|pair| NameIdPair {
                    name: name.clone(),
                    id: pair.id.clone(),
                })
        })
        .collect()
}

fn has_artist_fields(item_type: &str) -> bool {
    ["Audio", "MusicAlbum", "MusicVideo"]
        .iter()
        .any(|expected| is_item_type(item_type, expected))
}

fn has_album_artist_fields(item_type: &str) -> bool {
    ["Audio", "MusicAlbum"]
        .iter()
        .any(|expected| is_item_type(item_type, expected))
}

fn is_item_type(item_type: &str, expected: &str) -> bool {
    item_type.eq_ignore_ascii_case(expected)
        || item_type
            .rsplit('.')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(expected))
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

pub(crate) async fn child_counts_for_items(
    state: &AppState,
    items: &[base_item::Model],
    fields: BaseItemDtoFields,
    target_user_id: Uuid,
) -> Result<HashMap<Uuid, u64>, ApiError> {
    if !fields.wants_child_count() {
        return Ok(HashMap::new());
    }
    let parent_ids = items
        .iter()
        .filter(|item| item.is_folder)
        .map(|item| item.id)
        .collect::<Vec<_>>();
    if parent_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let user = state.users.get(target_user_id).await?;
    let configuration: UserConfiguration =
        serde_json::from_value(user.preferences).unwrap_or_default();
    Ok(state
        .base_items
        .dto_child_counts(&parent_ids, configuration.display_missing_episodes)
        .await?)
}

pub(crate) async fn recursive_item_counts_for_items(
    state: &AppState,
    items: &[base_item::Model],
    fields: BaseItemDtoFields,
    target_user_id: Uuid,
) -> Result<HashMap<Uuid, u64>, ApiError> {
    if !fields.wants_recursive_item_count() {
        return Ok(HashMap::new());
    }
    let parent_ids = items
        .iter()
        .filter(|item| item.is_folder)
        .map(|item| item.id)
        .collect::<Vec<_>>();
    if parent_ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(state
        .user_library
        .recursive_item_counts(target_user_id, &parent_ids)
        .await?)
}

pub(crate) fn attach_child_count(dto: &mut BaseItemDto, child_count: Option<u64>) {
    if let Some(child_count) = child_count {
        dto.child_count = Some(child_count);
    }
}

pub(crate) fn attach_recursive_item_count(
    dto: &mut BaseItemDto,
    recursive_item_count: Option<u64>,
) {
    if let Some(recursive_item_count) = recursive_item_count {
        dto.recursive_item_count = Some(recursive_item_count);
    }
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

    if fields.media_sources {
        let source_streams = if fields.media_streams {
            // ALLOW: the response schema requires owned streams in both projections.
            media_streams.clone()
        } else {
            std::mem::take(&mut media_streams)
        };
        if let Some(source) = media_source_from_dto(
            dto,
            source_streams,
            media_attachments,
            default_audio_stream_index,
            default_subtitle_stream_index,
            false,
            None,
        ) {
            dto.media_sources = Some(vec![source]);
        }
    }
    if fields.media_streams {
        dto.media_streams = Some(media_streams);
    }
}

fn media_source_from_dto(
    dto: &BaseItemDto,
    media_streams: Vec<MediaStream>,
    media_attachments: Vec<MediaAttachment>,
    default_audio_stream_index: Option<i32>,
    default_subtitle_stream_index: Option<i32>,
    has_local_alternates: bool,
    common_prefix: Option<&str>,
) -> Option<MediaSourceInfo> {
    if dto.is_folder || !is_media_source_item(dto) {
        return None;
    }

    let path = dto.media_source_path.clone().or_else(|| dto.path.clone());
    let protocol = path
        .as_deref()
        .map(media_protocol_from_path)
        .unwrap_or(MediaProtocol::File);
    let name = dto
        .path
        .as_deref()
        .or(path.as_deref())
        .map(|path| get_media_source_name(path, has_local_alternates, common_prefix))
        .or_else(|| dto.name.clone());
    let container =
        projected_media_source_container(dto.media_source_container.as_deref(), path.as_deref());
    let bitrate = dto
        .media_source_bitrate
        .or_else(|| infer_total_bitrate(&media_streams));
    let source_type = if path.is_some() {
        MediaSourceType::Default
    } else {
        MediaSourceType::Placeholder
    };
    Some(MediaSourceInfo {
        id: Some(dto.id.clone()),
        protocol,
        path,
        name,
        container,
        bitrate,
        size: dto.media_source_size,
        source_type,
        is_remote: protocol != MediaProtocol::File,
        run_time_ticks: dto.run_time_ticks,
        video_type: is_video_item(dto).then_some(VideoType::VideoFile),
        iso_type: dto.iso_type,
        video_3d_format: dto.video_3d_format,
        timestamp: dto.media_source_timestamp,
        media_streams,
        media_attachments,
        default_audio_stream_index,
        default_subtitle_stream_index,
        etag: (protocol == MediaProtocol::File)
            .then(|| dto.media_source_etag.clone())
            .flatten(),
        ..MediaSourceInfo::default()
    })
}

fn infer_total_bitrate(media_streams: &[MediaStream]) -> Option<i32> {
    let bitrate = media_streams
        .iter()
        .filter(|stream| !stream.is_external)
        .filter_map(|stream| stream.bit_rate)
        .map(i64::from)
        .sum::<i64>();
    if bitrate > 0 {
        i32::try_from(bitrate).ok()
    } else {
        None
    }
}

fn media_protocol_from_path(path: &str) -> MediaProtocol {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        MediaProtocol::Http
    } else if lower.starts_with("rtmp://") {
        MediaProtocol::Rtmp
    } else if lower.starts_with("rtsp://") {
        MediaProtocol::Rtsp
    } else if lower.starts_with("udp://") {
        MediaProtocol::Udp
    } else if lower.starts_with("rtp://") {
        MediaProtocol::Rtp
    } else if lower.starts_with("ftp://") {
        MediaProtocol::Ftp
    } else {
        MediaProtocol::File
    }
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

    let audio_stream_position = default_audio_stream_index.and_then(|index| {
        media_streams.iter().position(|stream| {
            stream.stream_type == MediaStreamType::Audio && stream.index == index
        })
    });
    let audio_language =
        audio_stream_position.and_then(|position| media_streams[position].language.take());
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
    if let Some(position) = audio_stream_position {
        media_streams[position].language = audio_language;
    }

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
            // ALLOW: cache lookup returns a shared culture while the response owns its aliases.
            vec![culture.name.clone()]
        } else {
            // ALLOW: cache lookup returns a shared culture while the response owns its aliases.
            culture.three_letter_iso_language_names.clone()
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
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| extension.to_ascii_lowercase())
}

fn normalize_media_source_container(container: Option<&str>, path: Option<&str>) -> Option<String> {
    let path_container = path.and_then(media_container_from_path);
    let mut containers = container?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let first = containers.next()?;
    if path_container.as_ref().is_some_and(|path_container| {
        first.eq_ignore_ascii_case(path_container)
            || containers.any(|container| container.eq_ignore_ascii_case(path_container))
    }) {
        path_container
    } else {
        Some(first)
    }
}

fn projected_media_source_container(container: Option<&str>, path: Option<&str>) -> Option<String> {
    normalize_media_source_container(container, path)
        .or_else(|| path.and_then(media_container_from_path))
}

pub(crate) fn music_genre_to_dto(
    genre: MusicGenre,
    server_id: &str,
    include_item_counts: bool,
) -> BaseItemDto {
    let presentation_unique_key = Some(format!("MusicGenre-{}", genre.name));
    let counts = genre.counts;
    let item_count = genre.item_count;
    let name = genre.name;
    let mut dto = BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as separate owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: genre.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "MusicGenre".to_owned(),
        etag: genre.id.simple().to_string(),
        date_created: None,
        sort_name: Some(name),
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
        presentation_unique_key,
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
    };
    apply_item_value_counts(
        &mut dto,
        counts,
        item_count,
        "MusicGenre",
        include_item_counts,
    );
    dto
}

pub(crate) fn genre_to_dto(
    genre: Genre,
    server_id: &str,
    include_item_counts: bool,
) -> BaseItemDto {
    let (item_type, presentation_prefix) = match genre.kind {
        GenreKind::Genre => ("Genre", "Genre"),
        GenreKind::MusicGenre => ("MusicGenre", "MusicGenre"),
    };
    let presentation_unique_key = Some(format!("{presentation_prefix}-{}", genre.name));
    let counts = genre.counts;
    let item_count = genre.item_count;
    let name = genre.name;
    let mut dto = BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as separate owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: genre.id.simple().to_string(),
        playlist_item_id: None,
        item_type: item_type.to_owned(),
        etag: genre.id.simple().to_string(),
        date_created: None,
        sort_name: Some(name),
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
        presentation_unique_key,
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
    };
    apply_item_value_counts(&mut dto, counts, item_count, item_type, include_item_counts);
    dto
}

pub(crate) fn studio_to_dto(
    studio: Studio,
    server_id: &str,
    include_item_counts: bool,
) -> BaseItemDto {
    let presentation_unique_key = Some(format!("Studio-{}", studio.name));
    let counts = studio.counts;
    let item_count = studio.item_count;
    let name = studio.name;
    let mut dto = BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as separate owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: studio.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "Studio".to_owned(),
        etag: studio.id.simple().to_string(),
        date_created: None,
        sort_name: Some(name),
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
        presentation_unique_key,
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
    };
    apply_item_value_counts(&mut dto, counts, item_count, "Studio", include_item_counts);
    dto
}

pub(crate) fn artist_to_dto(
    artist: Artist,
    server_id: &str,
    include_item_counts: bool,
) -> BaseItemDto {
    let presentation_unique_key = Some(format!("Artist-{}", artist.name));
    let counts = artist.counts;
    let item_count = artist.item_count;
    let name = artist.name;
    let mut dto = BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as separate owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: artist.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "MusicArtist".to_owned(),
        etag: artist.id.simple().to_string(),
        date_created: None,
        sort_name: Some(name),
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
        presentation_unique_key,
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
    };
    apply_item_value_counts(
        &mut dto,
        counts,
        item_count,
        "MusicArtist",
        include_item_counts,
    );
    dto
}

fn apply_item_value_counts(
    dto: &mut BaseItemDto,
    counts: ItemValueCounts,
    item_count: u64,
    item_type: &str,
    include_item_counts: bool,
) {
    if !include_item_counts {
        return;
    }
    dto.child_count = Some(item_count);
    dto.album_count = Some(counts.album_count);
    dto.music_video_count = Some(counts.music_video_count);
    dto.song_count = Some(counts.song_count);
    if item_type == "MusicArtist" {
        return;
    }
    dto.artist_count = Some(counts.artist_count);
    if item_type == "MusicGenre" {
        return;
    }
    dto.episode_count = Some(counts.episode_count);
    dto.movie_count = Some(counts.movie_count);
    dto.program_count = Some(counts.program_count);
    dto.series_count = Some(counts.series_count);
    dto.trailer_count = Some(counts.trailer_count);
}

pub(crate) fn person_to_dto(person: Person, server_id: &str) -> BaseItemDto {
    let model = person.model;
    let presentation_unique_key = Some(format!("Person-{}", model.name));
    let provider_ids = provider_ids_from_value(&model.provider_ids);
    let name = model.name;
    BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as separate owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: model.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "Person".to_owned(),
        etag: model.row_version.to_string(),
        date_created: Some(model.date_created.to_rfc3339()),
        sort_name: Some(name),
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
        presentation_unique_key,
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids,
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

pub(crate) fn year_to_dto(year: Year, server_id: &str) -> BaseItemDto {
    let presentation_unique_key = Some(format!("Year-{}", year.name));
    let name = year.name;
    BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as separate owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: year.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "Year".to_owned(),
        etag: year.id.simple().to_string(),
        date_created: None,
        sort_name: Some(name),
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
        presentation_unique_key,
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

fn metadata_api_datetime(data: Option<&Value>, keys: &[&str]) -> Option<String> {
    let value = metadata_string(data, keys)?;
    let value = if let Ok(value) = chrono::DateTime::parse_from_rfc3339(&value) {
        value.with_timezone(&Utc)
    } else {
        NaiveDate::parse_from_str(&value, "%Y-%m-%d")
            .ok()?
            .and_hms_opt(0, 0, 0)?
            .and_utc()
    };
    Some(value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn person_kind_from_name(value: &str) -> PersonKind {
    match value.trim().to_ascii_lowercase().as_str() {
        "actor" => PersonKind::Actor,
        "director" => PersonKind::Director,
        "composer" => PersonKind::Composer,
        "writer" => PersonKind::Writer,
        "gueststar" => PersonKind::GuestStar,
        "producer" => PersonKind::Producer,
        "conductor" => PersonKind::Conductor,
        "lyricist" => PersonKind::Lyricist,
        "arranger" => PersonKind::Arranger,
        "engineer" => PersonKind::Engineer,
        "mixer" => PersonKind::Mixer,
        "remixer" => PersonKind::Remixer,
        "creator" => PersonKind::Creator,
        "artist" => PersonKind::Artist,
        "albumartist" => PersonKind::AlbumArtist,
        "author" => PersonKind::Author,
        "illustrator" => PersonKind::Illustrator,
        "penciller" => PersonKind::Penciller,
        "inker" => PersonKind::Inker,
        "colorist" => PersonKind::Colorist,
        "letterer" => PersonKind::Letterer,
        "coverartist" => PersonKind::CoverArtist,
        "editor" => PersonKind::Editor,
        "translator" => PersonKind::Translator,
        "narrator" => PersonKind::Narrator,
        _ => PersonKind::Unknown,
    }
}

fn metadata_f64(data: Option<&Value>, keys: &[&str]) -> Option<f64> {
    metadata_value(data, keys).and_then(|value| value.as_f64())
}

fn metadata_i32(data: Option<&Value>, keys: &[&str]) -> Option<i32> {
    metadata_value(data, keys)
        .and_then(|value| value.as_i64().and_then(|value| i32::try_from(value).ok()))
}

fn metadata_i64(data: Option<&Value>, keys: &[&str]) -> Option<i64> {
    metadata_value(data, keys).and_then(|value| value.as_i64())
}

fn media_source_etag(date_modified: DateTime<Utc>) -> Option<String> {
    const UNIX_EPOCH_DOTNET_TICKS: i64 = 621_355_968_000_000_000;
    let ticks = date_modified
        .timestamp()
        .checked_mul(10_000_000)?
        .checked_add(i64::from(date_modified.timestamp_subsec_nanos() / 100))?
        .checked_add(UNIX_EPOCH_DOTNET_TICKS)?;
    let mut hasher = Md5::new();
    for unit in ticks.to_string().encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    Some(
        Uuid::from_bytes_le(hasher.finalize().into())
            .simple()
            .to_string(),
    )
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

const MEDIA_TYPES: &[&str] = &["Unknown", "Video", "Audio", "Photo", "Book"];
const COLLECTION_TYPES: &[&str] = &[
    "unknown",
    "movies",
    "tvshows",
    "music",
    "musicvideos",
    "trailers",
    "homevideos",
    "boxsets",
    "books",
    "photos",
    "livetv",
    "playlists",
    "folders",
];
const EXTRA_TYPES: &[&str] = &[
    "Unknown",
    "Clip",
    "Trailer",
    "BehindTheScenes",
    "DeletedScene",
    "Interview",
    "Scene",
    "Sample",
    "ThemeSong",
    "ThemeVideo",
    "Featurette",
    "Short",
];
const AIR_DAYS: &[&str] = &[
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const ISO_TYPES: &[(&str, IsoType)] = &[("Dvd", IsoType::Dvd), ("BluRay", IsoType::BluRay)];
const VIDEO_3D_FORMATS: &[(&str, Video3DFormat)] = &[
    ("HalfSideBySide", Video3DFormat::HalfSideBySide),
    ("FullSideBySide", Video3DFormat::FullSideBySide),
    ("FullTopAndBottom", Video3DFormat::FullTopAndBottom),
    ("HalfTopAndBottom", Video3DFormat::HalfTopAndBottom),
    ("MVC", Video3DFormat::MVC),
];
const TRANSPORT_STREAM_TIMESTAMPS: &[(&str, TransportStreamTimestamp)] = &[
    ("None", TransportStreamTimestamp::None),
    ("Zero", TransportStreamTimestamp::Zero),
    ("Valid", TransportStreamTimestamp::Valid),
];

fn metadata_enum<T: Copy>(
    data: Option<&Value>,
    keys: &[&str],
    variants: &[(&str, T)],
) -> Option<T> {
    let value = metadata_value(data, keys)?;
    match value {
        Value::String(value) => variants
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(value.trim()))
            .map(|(_, variant)| *variant)
            .or_else(|| {
                value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| variants.get(index))
                    .map(|(_, variant)| *variant)
            }),
        Value::Number(value) => value
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| variants.get(index))
            .map(|(_, variant)| *variant),
        _ => None,
    }
}

fn canonical_enum(value: &str, variants: &[&str]) -> Option<String> {
    variants
        .iter()
        .find(|variant| variant.eq_ignore_ascii_case(value.trim()))
        .map(|variant| (*variant).to_owned())
}

fn canonical_enum_or(value: &str, variants: &[&str], fallback: &str) -> String {
    canonical_enum(value, variants).unwrap_or_else(|| fallback.to_owned())
}

fn metadata_enum_strings(data: Option<&Value>, keys: &[&str], variants: &[&str]) -> Vec<String> {
    metadata_strings(data, keys)
        .into_iter()
        .filter_map(|value| canonical_enum(&value, variants))
        .collect()
}

fn metadata_provider_ids(data: Option<&Value>) -> Option<HashMap<String, String>> {
    metadata_value(data, &["ProviderIds", "provider_ids"])
        .as_ref()
        .and_then(provider_ids_from_value)
}

fn provider_ids_from_value(value: &Value) -> Option<HashMap<String, String>> {
    let object = value.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(key, value)| {
                let value = match value {
                    Value::String(value) if !value.is_empty() => value.clone(),
                    Value::Number(value) => value.to_string(),
                    _ => return None,
                };
                Some((key.clone(), value))
            })
            .collect(),
    )
}

fn metadata_taglines(data: Option<&Value>) -> Vec<String> {
    let taglines = metadata_strings(data, &["Taglines", "taglines"]);
    if taglines.is_empty() {
        metadata_string(data, &["Tagline", "tagline"])
            .into_iter()
            .collect()
    } else {
        taglines
    }
}

fn metadata_remote_trailers(data: Option<&Value>) -> Vec<MediaUrl> {
    metadata_value(data, &["RemoteTrailers", "remote_trailers"])
        .and_then(|value| value.as_array().cloned())
        .map(|values| {
            values
                .into_iter()
                .filter_map(|value| match value {
                    Value::String(url) if !url.is_empty() => Some(MediaUrl {
                        url: Some(url),
                        name: None,
                    }),
                    Value::Object(object) => object
                        .get("Url")
                        .or_else(|| object.get("url"))
                        .and_then(Value::as_str)
                        .filter(|url| !url.is_empty())
                        .map(|url| MediaUrl {
                            url: Some(url.to_owned()),
                            name: object
                                .get("Name")
                                .or_else(|| object.get("name"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn person_serialization_includes_primary_image_tag() {
        let person = BaseItemPerson {
            name: "Actor".to_owned(),
            id: "person-id".to_owned(),
            role: "Lead".to_owned(),
            person_type: PersonKind::Actor,
            primary_image_tag: Some("image-tag".to_owned()),
        };

        assert_eq!(
            serde_json::to_value(person).unwrap(),
            json!({
                "Name": "Actor",
                "Id": "person-id",
                "Role": "Lead",
                "Type": "Actor",
                "PrimaryImageTag": "image-tag"
            })
        );
    }

    #[test]
    fn person_kind_projection_is_case_insensitive_and_defaults_to_unknown() {
        assert_eq!(person_kind_from_name("gueststar"), PersonKind::GuestStar);
        assert_eq!(
            person_kind_from_name(" Cinematographer "),
            PersonKind::Unknown
        );
    }

    #[test]
    fn base_item_dto_uses_official_acronym_and_legacy_property_names() {
        let dto = BaseItemDto {
            video_3d_format: Some(Video3DFormat::MVC),
            iso_type: Some(IsoType::BluRay),
            is_locked: Some(true),
            parent_backdrop_image_item_id: Some("parent".to_owned()),
            ..BaseItemDto::default()
        };
        let value = serde_json::to_value(dto).unwrap();

        assert_eq!(value["Video3DFormat"], "MVC");
        assert_eq!(value["IsoType"], "BluRay");
        assert_eq!(value["LockData"], true);
        assert_eq!(value["ParentBackdropItemId"], "parent");
        assert!(value.get("Video3dFormat").is_none());
        assert!(value.get("IsLocked").is_none());
        assert!(value.get("ParentBackdropImageItemId").is_none());
    }

    #[test]
    fn item_to_dto_projects_persisted_metadata_json() {
        let item = base_item::Model {
            id: Uuid::new_v4(),
            item_type: "Movie".to_owned(),
            data: Some(json!({
                "CommunityRating": 8.5,
                "CriticRating": 7.0,
                "OriginalTitle": "Original",
                "OriginalLanguage": "Japanese",
                "SeriesName": "Example Series",
                "SeasonName": "Season 2",
                "Tagline": "Tag",
                "Status": "Ended",
                "IsLocked": true,
                "Width": 1920,
                "Height": 1080,
                "Bitrate": 5_500_000,
                "Container": "mkv,webm",
                "Size": 12345,
                "ExtraType": "behindthescenes",
                "AirDays": ["monday", "Funday", "Friday"],
                "EndDate": "2020-01-02",
                "Video3DFormat": "mvc",
                "IsoType": 1,
                "Timestamp": "2",
                "ProductionLocations": ["Los Angeles"],
                "ProviderIds": {
                    "Tmdb": 42,
                    "Imdb": "tt123",
                    "Missing": null,
                    "Nested": {"Id": "invalid"}
                },
                "RemoteTrailers": [
                    "https://trailers.example/legacy",
                    {"Name": "Official Trailer", "Url": "https://trailers.example/official"}
                ],
                "StrmTarget": "/CloudNAS/Movie/movie.mkv"
            })),
            path: Some("/library/Movie.strm".to_owned()),
            parent_id: None,
            top_parent_id: None,
            name: Some("Movie".to_owned()),
            clean_name: None,
            sort_name: None,
            media_type: Some("video".to_owned()),
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
        assert_eq!(dto.media_source_bitrate, Some(5_500_000));
        assert_eq!(dto.media_source_container.as_deref(), Some("mkv,webm"));
        assert_eq!(dto.media_source_size, Some(12_345));
        assert_eq!(
            dto.media_source_etag.as_deref(),
            Some("e19f5b6165c1331b55b7c60254e8695a")
        );
        assert_eq!(dto.original_title.as_deref(), Some("Original"));
        assert_eq!(dto.original_language.as_deref(), Some("Japanese"));
        assert_eq!(dto.series_name.as_deref(), Some("Example Series"));
        assert_eq!(dto.season_name.as_deref(), Some("Season 2"));
        assert_eq!(dto.taglines, ["Tag"]);
        assert_eq!(
            dto.provider_ids,
            Some(HashMap::from([
                ("Imdb".to_owned(), "tt123".to_owned()),
                ("Tmdb".to_owned(), "42".to_owned()),
            ]))
        );
        assert_eq!(
            dto.remote_trailers,
            [
                MediaUrl {
                    url: Some("https://trailers.example/legacy".to_owned()),
                    name: None,
                },
                MediaUrl {
                    url: Some("https://trailers.example/official".to_owned()),
                    name: Some("Official Trailer".to_owned()),
                }
            ]
        );
        assert_eq!(dto.status.as_deref(), Some("Ended"));
        assert_eq!(dto.is_locked, Some(true));
        assert_eq!(dto.width, Some(1920));
        assert_eq!(dto.height, Some(1080));
        assert_eq!(dto.media_type.as_deref(), Some("Video"));
        assert_eq!(dto.extra_type.as_deref(), Some("BehindTheScenes"));
        assert_eq!(dto.air_days, ["Monday", "Friday"]);
        assert_eq!(dto.end_date.as_deref(), Some("2020-01-02T00:00:00.000Z"));
        assert_eq!(dto.video_3d_format, Some(Video3DFormat::MVC));
        assert_eq!(dto.iso_type, Some(IsoType::BluRay));
        assert_eq!(dto.production_locations, ["Los Angeles"]);
        assert_eq!(dto.official_rating.as_deref(), Some("PG-13"));
        assert_eq!(dto.path.as_deref(), Some("/library/Movie.strm"));
        let source =
            media_source_from_dto(&dto, Vec::new(), Vec::new(), None, None, false, None).unwrap();
        assert_eq!(source.path.as_deref(), Some("/CloudNAS/Movie/movie.mkv"));
        assert_eq!(source.name.as_deref(), Some("Movie"));
        assert_eq!(source.bitrate, Some(5_500_000));
        assert_eq!(source.container.as_deref(), Some("mkv"));
        assert_eq!(source.size, Some(12_345));
        assert_eq!(source.video_type, Some(VideoType::VideoFile));
        assert_eq!(source.iso_type, Some(IsoType::BluRay));
        assert_eq!(source.video_3d_format, Some(Video3DFormat::MVC));
        assert_eq!(source.timestamp, Some(TransportStreamTimestamp::Valid));
        assert_eq!(
            source.etag.as_deref(),
            Some("e19f5b6165c1331b55b7c60254e8695a")
        );
        assert_eq!(source.protocol, MediaProtocol::File);
        assert!(!source.is_remote);
        assert!(
            serde_json::to_value(&dto)
                .unwrap()
                .get("MediaSourcePath")
                .is_none()
        );
        let json = serde_json::to_value(&dto).unwrap();
        assert!(json.get("Tagline").is_none());
        assert_eq!(json["Taglines"], json!(["Tag"]));
        assert_eq!(
            json["RemoteTrailers"],
            json!([
                {"Url": "https://trailers.example/legacy"},
                {"Name": "Official Trailer", "Url": "https://trailers.example/official"}
            ])
        );
        assert_eq!(json["EndDate"], "2020-01-02T00:00:00.000Z");
    }

    #[test]
    fn persisted_media_enums_accept_names_numbers_and_numeric_strings() {
        let data = json!({
            "IsoType": "dvd",
            "Video3DFormat": 3,
            "Timestamp": "1"
        });

        assert_eq!(
            metadata_enum(Some(&data), &["IsoType"], ISO_TYPES),
            Some(IsoType::Dvd)
        );
        assert_eq!(
            metadata_enum(Some(&data), &["Video3DFormat"], VIDEO_3D_FORMATS),
            Some(Video3DFormat::HalfTopAndBottom)
        );
        assert_eq!(
            metadata_enum(Some(&data), &["Timestamp"], TRANSPORT_STREAM_TIMESTAMPS),
            Some(TransportStreamTimestamp::Zero)
        );
        assert_eq!(
            metadata_enum(
                Some(&json!({"Timestamp": 9})),
                &["Timestamp"],
                TRANSPORT_STREAM_TIMESTAMPS
            ),
            None
        );
    }

    #[test]
    fn media_source_bitrate_inference_ignores_external_streams_and_overflow() {
        let streams = [
            MediaStream {
                bit_rate: Some(4_000_000),
                ..MediaStream::default()
            },
            MediaStream {
                bit_rate: Some(192_000),
                ..MediaStream::default()
            },
            MediaStream {
                bit_rate: Some(10_000_000),
                is_external: true,
                ..MediaStream::default()
            },
        ];
        assert_eq!(infer_total_bitrate(&streams), Some(4_192_000));

        let overflow = [
            MediaStream {
                bit_rate: Some(i32::MAX),
                ..MediaStream::default()
            },
            MediaStream {
                bit_rate: Some(1),
                ..MediaStream::default()
            },
        ];
        assert_eq!(infer_total_bitrate(&overflow), None);
    }

    #[test]
    fn metadata_api_datetime_normalizes_offsets_and_rejects_invalid_values() {
        assert_eq!(
            metadata_api_datetime(
                Some(&json!({"EndDate": "2020-01-02T03:04:05.123+08:00"})),
                &["EndDate"]
            )
            .as_deref(),
            Some("2020-01-01T19:04:05.123Z")
        );
        assert_eq!(
            metadata_api_datetime(Some(&json!({"EndDate": "not-a-date"})), &["EndDate"]),
            None
        );
    }

    #[test]
    fn strm_http_target_is_a_remote_http_media_source() {
        let dto = BaseItemDto {
            id: "item".to_owned(),
            item_type: "Movie".to_owned(),
            path: Some("/library/Cloud Movie.strm".to_owned()),
            media_source_path: Some(
                "https://media.example/Movie.MP4?token=secret.mkv#fragment".to_owned(),
            ),
            ..BaseItemDto::default()
        };

        let source =
            media_source_from_dto(&dto, Vec::new(), Vec::new(), None, None, false, None).unwrap();

        assert_eq!(source.protocol, MediaProtocol::Http);
        assert!(source.is_remote);
        assert_eq!(source.container.as_deref(), Some("mp4"));
        assert_eq!(source.name.as_deref(), Some("Cloud Movie"));
        assert_eq!(source.etag, None);
    }

    #[test]
    fn pathless_video_source_is_an_official_placeholder() {
        let dto = BaseItemDto {
            id: "item".to_owned(),
            item_type: "Movie".to_owned(),
            ..BaseItemDto::default()
        };
        let source =
            media_source_from_dto(&dto, Vec::new(), Vec::new(), None, None, false, None).unwrap();
        assert_eq!(source.source_type, MediaSourceType::Placeholder);
        assert_eq!(source.video_type, Some(VideoType::VideoFile));
    }

    #[test]
    fn persisted_media_source_container_wins_and_selects_matching_variant() {
        let dto = BaseItemDto {
            id: "item".to_owned(),
            item_type: "Movie".to_owned(),
            path: Some("/media/Movie.webm".to_owned()),
            media_source_container: Some("mkv, WEBM".to_owned()),
            ..BaseItemDto::default()
        };
        let source =
            media_source_from_dto(&dto, Vec::new(), Vec::new(), None, None, false, None).unwrap();
        assert_eq!(source.container.as_deref(), Some("webm"));

        let dto = BaseItemDto {
            path: Some("/media/Movie.unknown".to_owned()),
            media_source_container: Some("Matroska,webm".to_owned()),
            ..dto
        };
        let source =
            media_source_from_dto(&dto, Vec::new(), Vec::new(), None, None, false, None).unwrap();
        assert_eq!(source.container.as_deref(), Some("matroska"));
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
