use std::{fmt, marker::PhantomData, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_controller::{
    PlaybackProgressUpdate, PlaybackStartUpdate, PlaybackStopUpdate, parse_date_played,
};
use jellyfin_data::NewActivityLog;
use jellyfin_model::{PlayMethod, PlaybackOrder, PlayerStateInfo, RepeatMode, UserItemDataDto};
use serde::{Deserialize, de};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    authentication::{self, AuthenticatedIdentity, AuthenticatedSession},
    authorization, user_library,
};

#[derive(Debug, Default, Deserialize)]
pub struct MarkPlayedQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
    #[serde(default, rename = "datePlayed", alias = "DatePlayed")]
    pub date_played: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MarkUnplayedQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackProgressInfo {
    #[serde(alias = "canSeek", alias = "canseek")]
    pub can_seek: bool,
    #[serde(alias = "item")]
    pub item: Option<Value>,
    #[serde(alias = "itemId", alias = "itemid")]
    pub item_id: Uuid,
    #[serde(alias = "sessionId", alias = "sessionid")]
    pub session_id: Option<String>,
    #[serde(alias = "mediaSourceId", alias = "mediasourceid")]
    pub media_source_id: Option<String>,
    #[serde(
        alias = "positionTicks",
        alias = "positionticks",
        deserialize_with = "deserialize_optional_i64"
    )]
    pub position_ticks: Option<i64>,
    #[serde(
        alias = "audioStreamIndex",
        alias = "audiostreamindex",
        deserialize_with = "deserialize_optional_i32"
    )]
    pub audio_stream_index: Option<i32>,
    #[serde(
        alias = "subtitleStreamIndex",
        alias = "subtitlestreamindex",
        deserialize_with = "deserialize_optional_i32"
    )]
    pub subtitle_stream_index: Option<i32>,
    #[serde(alias = "isPaused", alias = "ispaused")]
    pub is_paused: bool,
    #[serde(alias = "isMuted", alias = "ismuted")]
    pub is_muted: bool,
    #[serde(
        alias = "volumeLevel",
        alias = "volumelevel",
        deserialize_with = "deserialize_optional_i32"
    )]
    pub volume_level: Option<i32>,
    #[serde(
        alias = "playMethod",
        alias = "playmethod",
        deserialize_with = "deserialize_optional_compat_enum"
    )]
    pub play_method: Option<PlayMethod>,
    #[serde(alias = "liveStreamId", alias = "livestreamid")]
    pub live_stream_id: Option<String>,
    #[serde(alias = "playSessionId", alias = "playsessionid")]
    pub play_session_id: Option<String>,
    #[serde(
        alias = "repeatMode",
        alias = "repeatmode",
        deserialize_with = "deserialize_compat_enum"
    )]
    pub repeat_mode: RepeatMode,
    #[serde(
        alias = "playbackOrder",
        alias = "playbackorder",
        deserialize_with = "deserialize_compat_enum"
    )]
    pub playback_order: PlaybackOrder,
    #[serde(alias = "nowPlayingQueue", alias = "nowplayingqueue")]
    pub now_playing_queue: Option<Vec<Value>>,
    #[serde(alias = "playlistItemId", alias = "playlistitemid")]
    pub playlist_item_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackStartInfo {
    #[serde(alias = "canSeek", alias = "canseek")]
    pub can_seek: bool,
    #[serde(alias = "item")]
    pub item: Option<Value>,
    #[serde(alias = "itemId", alias = "itemid")]
    pub item_id: Uuid,
    #[serde(alias = "sessionId", alias = "sessionid")]
    pub session_id: Option<String>,
    #[serde(alias = "mediaSourceId", alias = "mediasourceid")]
    pub media_source_id: Option<String>,
    #[serde(
        alias = "positionTicks",
        alias = "positionticks",
        deserialize_with = "deserialize_optional_i64"
    )]
    pub position_ticks: Option<i64>,
    #[serde(
        alias = "audioStreamIndex",
        alias = "audiostreamindex",
        deserialize_with = "deserialize_optional_i32"
    )]
    pub audio_stream_index: Option<i32>,
    #[serde(
        alias = "subtitleStreamIndex",
        alias = "subtitlestreamindex",
        deserialize_with = "deserialize_optional_i32"
    )]
    pub subtitle_stream_index: Option<i32>,
    #[serde(alias = "isPaused", alias = "ispaused")]
    pub is_paused: bool,
    #[serde(alias = "isMuted", alias = "ismuted")]
    pub is_muted: bool,
    #[serde(
        alias = "volumeLevel",
        alias = "volumelevel",
        deserialize_with = "deserialize_optional_i32"
    )]
    pub volume_level: Option<i32>,
    #[serde(
        alias = "playMethod",
        alias = "playmethod",
        deserialize_with = "deserialize_optional_compat_enum"
    )]
    pub play_method: Option<PlayMethod>,
    #[serde(alias = "liveStreamId", alias = "livestreamid")]
    pub live_stream_id: Option<String>,
    #[serde(alias = "playSessionId", alias = "playsessionid")]
    pub play_session_id: Option<String>,
    #[serde(
        alias = "repeatMode",
        alias = "repeatmode",
        deserialize_with = "deserialize_compat_enum"
    )]
    pub repeat_mode: RepeatMode,
    #[serde(
        alias = "playbackOrder",
        alias = "playbackorder",
        deserialize_with = "deserialize_compat_enum"
    )]
    pub playback_order: PlaybackOrder,
    #[serde(alias = "nowPlayingQueue", alias = "nowplayingqueue")]
    pub now_playing_queue: Option<Vec<Value>>,
    #[serde(alias = "playlistItemId", alias = "playlistitemid")]
    pub playlist_item_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackStopInfo {
    #[serde(alias = "item")]
    pub item: Option<Value>,
    #[serde(alias = "itemId", alias = "itemid")]
    pub item_id: Uuid,
    #[serde(alias = "sessionId", alias = "sessionid")]
    pub session_id: Option<String>,
    #[serde(alias = "mediaSourceId", alias = "mediasourceid")]
    pub media_source_id: Option<String>,
    #[serde(
        alias = "positionTicks",
        alias = "positionticks",
        deserialize_with = "deserialize_optional_i64"
    )]
    pub position_ticks: Option<i64>,
    #[serde(alias = "liveStreamId", alias = "livestreamid")]
    pub live_stream_id: Option<String>,
    #[serde(alias = "playSessionId", alias = "playsessionid")]
    pub play_session_id: Option<String>,
    #[serde(alias = "failed")]
    pub failed: bool,
    #[serde(alias = "nextMediaType", alias = "nextmediatype")]
    pub next_media_type: Option<String>,
    #[serde(alias = "playlistItemId", alias = "playlistitemid")]
    pub playlist_item_id: Option<String>,
    #[serde(alias = "nowPlayingQueue", alias = "nowplayingqueue")]
    pub now_playing_queue: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackStartQuery {
    #[serde(default, rename = "mediaSourceId", alias = "MediaSourceId")]
    pub media_source_id: Option<String>,
    #[serde(default, rename = "audioStreamIndex", alias = "AudioStreamIndex")]
    pub audio_stream_index: Option<i32>,
    #[serde(default, rename = "subtitleStreamIndex", alias = "SubtitleStreamIndex")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(
        default,
        rename = "playMethod",
        alias = "PlayMethod",
        deserialize_with = "deserialize_optional_compat_enum"
    )]
    pub play_method: Option<PlayMethod>,
    #[serde(default, rename = "liveStreamId", alias = "LiveStreamId")]
    pub live_stream_id: Option<String>,
    #[serde(default, rename = "playSessionId", alias = "PlaySessionId")]
    pub play_session_id: Option<String>,
    #[serde(default, rename = "canSeek", alias = "CanSeek")]
    pub can_seek: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackProgressQuery {
    #[serde(default, rename = "mediaSourceId", alias = "MediaSourceId")]
    pub media_source_id: Option<String>,
    #[serde(default, rename = "positionTicks", alias = "PositionTicks")]
    pub position_ticks: Option<i64>,
    #[serde(default, rename = "audioStreamIndex", alias = "AudioStreamIndex")]
    pub audio_stream_index: Option<i32>,
    #[serde(default, rename = "subtitleStreamIndex", alias = "SubtitleStreamIndex")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(default, rename = "volumeLevel", alias = "VolumeLevel")]
    pub volume_level: Option<i32>,
    #[serde(
        default,
        rename = "playMethod",
        alias = "PlayMethod",
        deserialize_with = "deserialize_optional_compat_enum"
    )]
    pub play_method: Option<PlayMethod>,
    #[serde(default, rename = "liveStreamId", alias = "LiveStreamId")]
    pub live_stream_id: Option<String>,
    #[serde(default, rename = "playSessionId", alias = "PlaySessionId")]
    pub play_session_id: Option<String>,
    #[serde(
        default,
        rename = "repeatMode",
        alias = "RepeatMode",
        deserialize_with = "deserialize_optional_compat_enum"
    )]
    pub repeat_mode: Option<RepeatMode>,
    #[serde(default, rename = "isPaused", alias = "IsPaused")]
    pub is_paused: bool,
    #[serde(default, rename = "isMuted", alias = "IsMuted")]
    pub is_muted: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackStopQuery {
    #[serde(default, rename = "mediaSourceId", alias = "MediaSourceId")]
    pub media_source_id: Option<String>,
    #[serde(default, rename = "positionTicks", alias = "PositionTicks")]
    pub position_ticks: Option<i64>,
    #[serde(default, rename = "nextMediaType", alias = "NextMediaType")]
    pub next_media_type: Option<String>,
    #[serde(default, rename = "liveStreamId", alias = "LiveStreamId")]
    pub live_stream_id: Option<String>,
    #[serde(default, rename = "playSessionId", alias = "PlaySessionId")]
    pub play_session_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlaybackPingQuery {
    #[serde(default, rename = "playSessionId", alias = "PlaySessionId")]
    pub play_session_id: Option<String>,
}

fn deserialize_optional_integer<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: TryFrom<i64> + std::str::FromStr,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_i64()
            .and_then(|value| T::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| de::Error::custom("integer is outside the supported range")),
        Some(Value::String(value)) => value
            .parse()
            .map(Some)
            .map_err(|_| de::Error::custom("invalid integer string")),
        Some(_) => Err(de::Error::custom("expected an integer or integer string")),
    }
}

fn deserialize_optional_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_integer(deserializer)
}

fn deserialize_optional_i32<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_integer(deserializer)
}

trait CompatibleEnum: Default + Sized {
    fn from_i64(value: i64) -> Option<Self>;
    fn from_name(value: &str) -> Option<Self>;
}

impl CompatibleEnum for PlayMethod {
    fn from_i64(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::Transcode),
            1 => Some(Self::DirectStream),
            2 => Some(Self::DirectPlay),
            _ => None,
        }
    }

    fn from_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("Transcode") {
            Some(Self::Transcode)
        } else if value.eq_ignore_ascii_case("DirectStream") {
            Some(Self::DirectStream)
        } else if value.eq_ignore_ascii_case("DirectPlay") {
            Some(Self::DirectPlay)
        } else {
            value.parse().ok().and_then(Self::from_i64)
        }
    }
}

impl CompatibleEnum for RepeatMode {
    fn from_i64(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::RepeatNone),
            1 => Some(Self::RepeatAll),
            2 => Some(Self::RepeatOne),
            _ => None,
        }
    }

    fn from_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("RepeatNone") {
            Some(Self::RepeatNone)
        } else if value.eq_ignore_ascii_case("RepeatAll") {
            Some(Self::RepeatAll)
        } else if value.eq_ignore_ascii_case("RepeatOne") {
            Some(Self::RepeatOne)
        } else {
            value.parse().ok().and_then(Self::from_i64)
        }
    }
}

impl CompatibleEnum for PlaybackOrder {
    fn from_i64(value: i64) -> Option<Self> {
        match value {
            0 => Some(Self::Default),
            1 => Some(Self::Shuffle),
            _ => None,
        }
    }

    fn from_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("Default") {
            Some(Self::Default)
        } else if value.eq_ignore_ascii_case("Shuffle") {
            Some(Self::Shuffle)
        } else {
            value.parse().ok().and_then(Self::from_i64)
        }
    }
}

struct CompatibleEnumVisitor<T>(PhantomData<T>);

impl<T: CompatibleEnum> de::Visitor<'_> for CompatibleEnumVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an enum name or its integer value")
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
        T::from_i64(value).ok_or_else(|| E::custom("unknown enum integer"))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        i64::try_from(value)
            .ok()
            .and_then(T::from_i64)
            .ok_or_else(|| E::custom("unknown enum integer"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        T::from_name(value).ok_or_else(|| E::custom("unknown enum name"))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(T::default())
    }
}

fn deserialize_compat_enum<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: CompatibleEnum,
{
    deserializer.deserialize_any(CompatibleEnumVisitor(PhantomData))
}

struct OptionalCompatibleEnumVisitor<T>(PhantomData<T>);

impl<'de, T: CompatibleEnum> de::Visitor<'de> for OptionalCompatibleEnumVisitor<T> {
    type Value = Option<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional enum name or its integer value")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer
            .deserialize_any(CompatibleEnumVisitor(PhantomData))
            .map(Some)
    }
}

fn deserialize_optional_compat_enum<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: CompatibleEnum,
{
    deserializer.deserialize_option(OptionalCompatibleEnumVisitor(PhantomData))
}

pub(crate) async fn mark_played_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<MarkPlayedQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_played_for(
        state,
        &uri,
        headers,
        query.user_id,
        item_id,
        query.date_played.as_deref(),
    )
    .await
}

pub(crate) async fn mark_unplayed_modern(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<MarkUnplayedQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_unplayed_for(state, &uri, headers, query.user_id, item_id).await
}

pub(crate) async fn mark_played(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<MarkPlayedQuery>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_played_for(
        state,
        &uri,
        headers,
        Some(user_id),
        item_id,
        query.date_played.as_deref(),
    )
    .await
}

async fn mark_played_for(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    date_played: Option<&str>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let date_played = date_played.map(parse_date_played).transpose()?;
    let update = state
        .playstate
        .mark_played_for_authorized_user(target_user_id, item_id, date_played)
        .await?;
    Ok(Json(update.into()))
}

pub(crate) async fn mark_unplayed(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    mark_unplayed_for(state, &uri, headers, Some(user_id), item_id).await
}

async fn mark_unplayed_for(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    let target_user_id = identity.target_user_id(requested_user_id)?;
    let update = state
        .playstate
        .mark_unplayed_for_authorized_user(target_user_id, item_id)
        .await?;
    Ok(Json(update.into()))
}

pub(crate) async fn report_playback_progress(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<PlaybackProgressInfo>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(progress) = request.map_err(|_| ApiError::InvalidRequest)?;
    report_playback_progress_for_current_session(state, &uri, headers, progress).await
}

pub(crate) async fn report_playback_start(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<PlaybackStartInfo>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(start) = request.map_err(|_| ApiError::InvalidRequest)?;
    report_playback_start_for_current_session(state, &uri, headers, start).await
}

pub(crate) async fn report_playback_stopped(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<PlaybackStopInfo>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(stop) = request.map_err(|_| ApiError::InvalidRequest)?;
    report_playback_stop_for_current_session(state, &uri, headers, stop).await
}

pub(crate) async fn ping_playback_session(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PlaybackPingQuery>,
) -> Result<StatusCode, ApiError> {
    if query.play_session_id.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::InvalidRequest);
    }
    authorization::require_default(&state, &headers, &uri).await?;
    if let Some(play_session_id) = query.play_session_id.as_deref() {
        state.transcode_jobs.ping(play_session_id, None);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn report_playback_start_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlaybackStartQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_start_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStartInfo {
            can_seek: query.can_seek,
            item_id,
            media_source_id: query.media_source_id,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            play_method: query.play_method,
            live_stream_id: query.live_stream_id,
            play_session_id: query.play_session_id,
            ..Default::default()
        },
    )
    .await
}

pub(crate) async fn report_playback_stopped_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlaybackStopQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_stop_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStopInfo {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            next_media_type: query.next_media_type,
            live_stream_id: query.live_stream_id,
            play_session_id: query.play_session_id,
            failed: false,
            ..Default::default()
        },
    )
    .await
}

pub(crate) async fn report_playback_start_legacy_for_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<PlaybackStartQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_start_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStartInfo {
            can_seek: query.can_seek,
            item_id,
            media_source_id: query.media_source_id,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            play_method: query.play_method,
            live_stream_id: query.live_stream_id,
            play_session_id: query.play_session_id,
            ..Default::default()
        },
    )
    .await
}

pub(crate) async fn report_playback_stopped_legacy_for_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<PlaybackStopQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_stop_for_current_session(
        state,
        &uri,
        headers,
        PlaybackStopInfo {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            next_media_type: query.next_media_type,
            live_stream_id: query.live_stream_id,
            play_session_id: query.play_session_id,
            failed: false,
            ..Default::default()
        },
    )
    .await
}

pub(crate) async fn report_playback_progress_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlaybackProgressQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_progress_for_current_session(
        state,
        &uri,
        headers,
        PlaybackProgressInfo {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            is_paused: query.is_paused,
            is_muted: query.is_muted,
            volume_level: query.volume_level,
            play_method: query.play_method,
            live_stream_id: query.live_stream_id,
            play_session_id: query.play_session_id,
            repeat_mode: query.repeat_mode.unwrap_or_default(),
            ..Default::default()
        },
    )
    .await
}

pub(crate) async fn report_playback_progress_legacy_for_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<PlaybackProgressQuery>,
) -> Result<StatusCode, ApiError> {
    report_playback_progress_for_current_session(
        state,
        &uri,
        headers,
        PlaybackProgressInfo {
            item_id,
            media_source_id: query.media_source_id,
            position_ticks: query.position_ticks,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            is_paused: query.is_paused,
            is_muted: query.is_muted,
            volume_level: query.volume_level,
            play_method: query.play_method,
            live_stream_id: query.live_stream_id,
            play_session_id: query.play_session_id,
            repeat_mode: query.repeat_mode.unwrap_or_default(),
            ..Default::default()
        },
    )
    .await
}

async fn report_playback_progress_for_current_session(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    info: PlaybackProgressInfo,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    if let AuthenticatedIdentity::Device(session) = identity {
        tracing::info!(
            device_id = %session.device.device_id,
            item_id = %info.item_id,
            position_ticks = ?info.position_ticks,
            play_method = ?info.play_method,
            paused = info.is_paused,
            "playback progress received",
        );
        record_device_playback_progress(&state, &session, info).await?;
        crate::websocket::broadcast_sessions(&state).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn report_playback_start_for_current_session(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    info: PlaybackStartInfo,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    if let AuthenticatedIdentity::Device(session) = identity {
        tracing::info!(
            device_id = %session.device.device_id,
            item_id = %info.item_id,
            position_ticks = ?info.position_ticks,
            play_method = ?info.play_method,
            "playback start received",
        );
        record_device_playback_start(&state, &session, info).await?;
        crate::websocket::broadcast_sessions(&state).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn report_playback_stop_for_current_session(
    state: Arc<AppState>,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    info: PlaybackStopInfo,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, uri).await?;
    if let AuthenticatedIdentity::Device(session) = identity {
        tracing::info!(
            device_id = %session.device.device_id,
            item_id = %info.item_id,
            position_ticks = ?info.position_ticks,
            failed = info.failed,
            "playback stop received",
        );
        record_device_playback_stop(&state, &session, info).await?;
        crate::websocket::broadcast_sessions(&state).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn record_device_playback_progress(
    state: &AppState,
    session: &AuthenticatedSession,
    info: PlaybackProgressInfo,
) -> Result<(), ApiError> {
    let update = PlaybackProgressUpdate::from(&info);
    state
        .playstate
        .report_playback_progress(&session.user, update.clone())
        .await?;
    for additional in additional_user_ids(&session.device.additional_users) {
        if let Ok(user) = state.users.get(additional).await {
            state
                .playstate
                .report_playback_progress(&user, update.clone())
                .await?;
        }
    }
    persist_session_playback_state(state, session, info).await?;
    Ok(())
}

async fn record_device_playback_start(
    state: &AppState,
    session: &AuthenticatedSession,
    info: PlaybackStartInfo,
) -> Result<(), ApiError> {
    let item_id = info.item_id;
    let update = PlaybackStartUpdate::from(&info);
    state
        .playstate
        .report_playback_start(&session.user, update.clone())
        .await?;
    for additional in additional_user_ids(&session.device.additional_users) {
        if let Ok(user) = state.users.get(additional).await {
            state
                .playstate
                .report_playback_start(&user, update.clone())
                .await?;
        }
    }
    persist_session_playback_state(state, session, info).await?;
    log_playback_activity(state, session, item_id, "start").await;
    Ok(())
}

async fn record_device_playback_stop(
    state: &AppState,
    session: &AuthenticatedSession,
    mut info: PlaybackStopInfo,
) -> Result<(), ApiError> {
    let item_id = info.item_id;
    if let Some(play_session_id) = info.play_session_id.as_deref() {
        state
            .transcode_jobs
            .stop_for_session(&session.device.device_id, play_session_id)
            .await;
    }
    let now_playing_queue = info.now_playing_queue.take().map(|queue| json!(queue));
    let playlist_item_id = info.playlist_item_id.take();
    let update = PlaybackStopUpdate::from(info);
    state
        .playstate
        .report_playback_stop(&session.user, update.clone())
        .await?;
    for additional in additional_user_ids(&session.device.additional_users) {
        if let Ok(user) = state.users.get(additional).await {
            state
                .playstate
                .report_playback_stop(&user, update.clone())
                .await?;
        }
    }
    state
        .devices
        .clear_playback_state(session.device.id, now_playing_queue, playlist_item_id)
        .await?;
    log_playback_activity(state, session, item_id, "stop").await;
    Ok(())
}

fn additional_user_ids(value: &Value) -> Vec<Uuid> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("UserId")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
        })
        .collect()
}

async fn log_playback_activity(
    state: &AppState,
    session: &AuthenticatedSession,
    item_id: Uuid,
    action: &str,
) {
    let Ok(Some(item)) = state.base_items.get(item_id).await else {
        return;
    };
    let activity_type = match action {
        "start" => match item.media_type.as_deref() {
            Some(media_type) if media_type.eq_ignore_ascii_case("Audio") => "AudioPlayback",
            Some(media_type) if media_type.eq_ignore_ascii_case("Video") => "VideoPlayback",
            _ => "Playback",
        },
        _ => match item.media_type.as_deref() {
            Some(media_type) if media_type.eq_ignore_ascii_case("Audio") => "AudioPlaybackStopped",
            Some(media_type) if media_type.eq_ignore_ascii_case("Video") => "VideoPlaybackStopped",
            _ => "PlaybackStopped",
        },
    };
    let name = item.name.as_deref().unwrap_or("Unknown");
    let mut entry = NewActivityLog::new(
        format!(
            "{} {} playing {}",
            session.user.username,
            if action == "start" {
                "started"
            } else {
                "stopped"
            },
            name
        ),
        activity_type,
        session.user.id,
    );
    entry.item_id = Some(item.id.simple().to_string());
    authentication::log_activity(state, entry);
}

async fn persist_session_playback_state<T>(
    state: &AppState,
    session: &AuthenticatedSession,
    mut info: T,
) -> Result<(), ApiError>
where
    T: PlaybackSessionState,
{
    let item_id = info.item_id();
    let now_playing_item = session_now_playing_item(state, item_id, info.take_item()).await?;
    let media_source_id = info
        .take_media_source_id()
        .filter(|value| !value.is_empty())
        .or_else(|| (!item_id.is_nil()).then(|| item_id.simple().to_string()));
    let play_state = PlayerStateInfo {
        position_ticks: info.position_ticks(),
        can_seek: info.can_seek(),
        is_paused: info.is_paused(),
        is_muted: info.is_muted(),
        volume_level: info.volume_level(),
        audio_stream_index: info.audio_stream_index(),
        subtitle_stream_index: info.subtitle_stream_index(),
        media_source_id,
        play_method: info.play_method(),
        repeat_mode: info.repeat_mode(),
        playback_order: info.playback_order(),
        live_stream_id: info.take_live_stream_id(),
    };
    let play_state = serde_json::to_value(play_state).map_err(|_| ApiError::Internal)?;
    // Official Jellyfin only applies NowPlayingQueue from a stop report. Start
    // and progress requests must not erase a queue maintained by session
    // control when the field is absent (or replace it when it is present).
    let playlist_item_id = info.take_playlist_item_id();
    if state
        .devices
        .update_playback_state(
            session.device.id,
            play_state,
            now_playing_item,
            None,
            playlist_item_id,
            info.is_paused(),
        )
        .await?
        != 1
    {
        return Err(ApiError::SessionNotFound);
    }
    Ok(())
}

async fn session_now_playing_item(
    state: &AppState,
    item_id: Uuid,
    reported_item: Option<Value>,
) -> Result<Option<Value>, ApiError> {
    if let Some(item) = reported_item.filter(|value| value.is_object()) {
        return Ok(Some(item));
    }
    if item_id.is_nil() {
        return Ok(None);
    }
    let Some(item) = state.base_items.get(item_id).await? else {
        return Ok(None);
    };
    let item = user_library::item_to_dto(item, state.server_id());
    Ok(Some(
        serde_json::to_value(item).map_err(|_| ApiError::Internal)?,
    ))
}

trait PlaybackSessionState {
    fn item_id(&self) -> Uuid;
    fn take_item(&mut self) -> Option<Value>;
    fn take_media_source_id(&mut self) -> Option<String>;
    fn position_ticks(&self) -> Option<i64>;
    fn audio_stream_index(&self) -> Option<i32>;
    fn subtitle_stream_index(&self) -> Option<i32>;
    fn can_seek(&self) -> bool;
    fn is_paused(&self) -> bool;
    fn is_muted(&self) -> bool;
    fn volume_level(&self) -> Option<i32>;
    fn play_method(&self) -> Option<PlayMethod>;
    fn take_live_stream_id(&mut self) -> Option<String>;
    fn repeat_mode(&self) -> RepeatMode;
    fn playback_order(&self) -> PlaybackOrder;
    fn take_playlist_item_id(&mut self) -> Option<String>;
}

impl PlaybackSessionState for PlaybackProgressInfo {
    fn item_id(&self) -> Uuid {
        self.item_id
    }
    fn take_item(&mut self) -> Option<Value> {
        self.item.take()
    }
    fn take_media_source_id(&mut self) -> Option<String> {
        self.media_source_id.take()
    }
    fn position_ticks(&self) -> Option<i64> {
        self.position_ticks
    }
    fn audio_stream_index(&self) -> Option<i32> {
        self.audio_stream_index
    }
    fn subtitle_stream_index(&self) -> Option<i32> {
        self.subtitle_stream_index
    }
    fn can_seek(&self) -> bool {
        self.can_seek
    }
    fn is_paused(&self) -> bool {
        self.is_paused
    }
    fn is_muted(&self) -> bool {
        self.is_muted
    }
    fn volume_level(&self) -> Option<i32> {
        self.volume_level
    }
    fn play_method(&self) -> Option<PlayMethod> {
        self.play_method
    }
    fn take_live_stream_id(&mut self) -> Option<String> {
        self.live_stream_id.take()
    }
    fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }
    fn playback_order(&self) -> PlaybackOrder {
        self.playback_order
    }
    fn take_playlist_item_id(&mut self) -> Option<String> {
        self.playlist_item_id.take()
    }
}

impl PlaybackSessionState for PlaybackStartInfo {
    fn item_id(&self) -> Uuid {
        self.item_id
    }
    fn take_item(&mut self) -> Option<Value> {
        self.item.take()
    }
    fn take_media_source_id(&mut self) -> Option<String> {
        self.media_source_id.take()
    }
    fn position_ticks(&self) -> Option<i64> {
        self.position_ticks
    }
    fn audio_stream_index(&self) -> Option<i32> {
        self.audio_stream_index
    }
    fn subtitle_stream_index(&self) -> Option<i32> {
        self.subtitle_stream_index
    }
    fn can_seek(&self) -> bool {
        self.can_seek
    }
    fn is_paused(&self) -> bool {
        self.is_paused
    }
    fn is_muted(&self) -> bool {
        self.is_muted
    }
    fn volume_level(&self) -> Option<i32> {
        self.volume_level
    }
    fn play_method(&self) -> Option<PlayMethod> {
        self.play_method
    }
    fn take_live_stream_id(&mut self) -> Option<String> {
        self.live_stream_id.take()
    }
    fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }
    fn playback_order(&self) -> PlaybackOrder {
        self.playback_order
    }
    fn take_playlist_item_id(&mut self) -> Option<String> {
        self.playlist_item_id.take()
    }
}

impl From<PlaybackProgressInfo> for PlaybackProgressUpdate {
    fn from(info: PlaybackProgressInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id,
            position_ticks: info.position_ticks,
            audio_stream_index: info.audio_stream_index,
            subtitle_stream_index: info.subtitle_stream_index,
        }
    }
}

impl From<&PlaybackProgressInfo> for PlaybackProgressUpdate {
    fn from(info: &PlaybackProgressInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id.clone(),
            position_ticks: info.position_ticks,
            audio_stream_index: info.audio_stream_index,
            subtitle_stream_index: info.subtitle_stream_index,
        }
    }
}

impl From<PlaybackStopInfo> for PlaybackStopUpdate {
    fn from(info: PlaybackStopInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id,
            position_ticks: info.position_ticks,
            failed: info.failed,
        }
    }
}

impl From<PlaybackStartInfo> for PlaybackStartUpdate {
    fn from(info: PlaybackStartInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id,
        }
    }
}

impl From<&PlaybackStartInfo> for PlaybackStartUpdate {
    fn from(info: &PlaybackStartInfo) -> Self {
        Self {
            item_id: info.item_id,
            media_source_id: info.media_source_id.clone(),
        }
    }
}
