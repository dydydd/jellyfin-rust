use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    Json,
    body::Body,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderValue, Response, header},
};
use jellyfin_model::{MediaProtocol, MediaSourceInfo};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

const DEFAULT_BITRATE_TEST_SIZE: i64 = 102_400;
const MAX_BITRATE_TEST_SIZE: i64 = 100_000_000;
const STREAM_BUFFER_SIZE: usize = 64 * 1024;
const REPEATING_BLOCK_SIZE: usize = 4 * 1024;
const OCTET_STREAM: HeaderValue = HeaderValue::from_static("application/octet-stream");
static REPEATING_BLOCK: [u8; REPEATING_BLOCK_SIZE] = bitrate_test_block();

#[derive(Debug, Default, Deserialize)]
pub(crate) struct BitrateTestQuery {
    size: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PlaybackInfoQuery {
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "mediaSourceId", alias = "MediaSourceId")]
    media_source_id: Option<String>,
    #[serde(rename = "maxStreamingBitrate", alias = "MaxStreamingBitrate")]
    max_streaming_bitrate: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct PlaybackInfoDto {
    user_id: Option<Uuid>,
    media_source_id: Option<String>,
    max_streaming_bitrate: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OpenLiveStreamQuery {
    #[serde(rename = "openToken", alias = "OpenToken")]
    open_token: Option<String>,
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "playSessionId", alias = "PlaySessionId")]
    play_session_id: Option<String>,
    #[serde(rename = "itemId", alias = "ItemId")]
    item_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct OpenLiveStreamDto {
    open_token: Option<String>,
    user_id: Option<Uuid>,
    play_session_id: Option<String>,
    item_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CloseLiveStreamQuery {
    #[serde(rename = "liveStreamId", alias = "LiveStreamId")]
    live_stream_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaybackInfoResponse {
    media_sources: Vec<MediaSourceInfo>,
    play_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct LiveStreamResponse {
    media_source: MediaSourceInfo,
}

pub(crate) async fn bitrate_test(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<BitrateTestQuery>,
) -> Result<Response<Body>, ApiError> {
    authentication::authenticated_session(&state, &headers).await?;
    let size = query.size.unwrap_or(DEFAULT_BITRATE_TEST_SIZE);
    if !(1..=MAX_BITRATE_TEST_SIZE).contains(&size) {
        return Err(ApiError::InvalidRequest);
    }
    let size = u64::try_from(size).map_err(|_| ApiError::InvalidRequest)?;
    let reader = RepeatingChunkReader::new(size);
    let stream = ReaderStream::with_capacity(reader, STREAM_BUFFER_SIZE);
    Response::builder()
        .header(header::CONTENT_TYPE, OCTET_STREAM)
        .header(header::CONTENT_LENGTH, size)
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError::Internal)
}

pub(crate) async fn get_playback_info(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<PlaybackInfoQuery>, QueryRejection>,
) -> Result<Json<PlaybackInfoResponse>, ApiError> {
    let identity = authentication::authenticated_session(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    playback_info(
        &state,
        &identity.user,
        query.user_id.unwrap_or(identity.user.id),
        item_id,
        query.media_source_id.as_deref(),
        None,
    )
    .await
    .map(Json)
}

pub(crate) async fn post_playback_info(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<PlaybackInfoQuery>, QueryRejection>,
    body: Result<Option<Json<PlaybackInfoDto>>, JsonRejection>,
) -> Result<Json<PlaybackInfoResponse>, ApiError> {
    let identity = authentication::authenticated_session(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let body = optional_playback_body(body)?;
    let target_user_id = query
        .user_id
        .or_else(|| body.as_ref().and_then(|body| body.user_id))
        .unwrap_or(identity.user.id);
    let media_source_id = query.media_source_id.as_deref().or_else(|| {
        body.as_ref()
            .and_then(|body| body.media_source_id.as_deref())
    });
    let max_streaming_bitrate = query
        .max_streaming_bitrate
        .or_else(|| body.as_ref().and_then(|body| body.max_streaming_bitrate));
    playback_info(
        &state,
        &identity.user,
        target_user_id,
        item_id,
        media_source_id,
        max_streaming_bitrate,
    )
    .await
    .map(Json)
}

pub(crate) async fn open_live_stream(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    query: Result<Query<OpenLiveStreamQuery>, QueryRejection>,
    body: Result<Option<Json<OpenLiveStreamDto>>, JsonRejection>,
) -> Result<Json<LiveStreamResponse>, ApiError> {
    let identity = authentication::authenticated_session(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let body = optional_open_live_stream_body(body)?;
    let target_user_id = query
        .user_id
        .or_else(|| body.as_ref().and_then(|body| body.user_id))
        .unwrap_or(identity.user.id);
    let item_id = query
        .item_id
        .or_else(|| body.as_ref().and_then(|body| body.item_id))
        .ok_or(ApiError::NotFound)?;
    let open_token = query
        .open_token
        .as_deref()
        .or_else(|| body.as_ref().and_then(|body| body.open_token.as_deref()));
    let play_session_id = query.play_session_id.as_deref().or_else(|| {
        body.as_ref()
            .and_then(|body| body.play_session_id.as_deref())
    });
    let mut media_source =
        media_source(&state, &identity.user, target_user_id, item_id, None).await?;
    media_source.requires_opening = false;
    media_source.requires_closing = true;
    media_source.live_stream_id = Some(live_stream_id(item_id, play_session_id, open_token));
    Ok(Json(LiveStreamResponse { media_source }))
}

pub(crate) async fn close_live_stream(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    query: Result<Query<CloseLiveStreamQuery>, QueryRejection>,
) -> Result<axum::http::StatusCode, ApiError> {
    authentication::authenticated_session(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    if query.live_stream_id.trim().is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

fn optional_playback_body(
    body: Result<Option<Json<PlaybackInfoDto>>, JsonRejection>,
) -> Result<Option<PlaybackInfoDto>, ApiError> {
    match body {
        Ok(Some(Json(body))) => Ok(Some(body)),
        Ok(None) => Ok(None),
        Err(JsonRejection::MissingJsonContentType(_)) => Ok(None),
        Err(_) => Err(ApiError::InvalidRequest),
    }
}

fn optional_open_live_stream_body(
    body: Result<Option<Json<OpenLiveStreamDto>>, JsonRejection>,
) -> Result<Option<OpenLiveStreamDto>, ApiError> {
    match body {
        Ok(Some(Json(body))) => Ok(Some(body)),
        Ok(None) => Ok(None),
        Err(JsonRejection::MissingJsonContentType(_)) => Ok(None),
        Err(_) => Err(ApiError::InvalidRequest),
    }
}

async fn playback_info(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    item_id: Uuid,
    media_source_id: Option<&str>,
    max_streaming_bitrate: Option<i32>,
) -> Result<PlaybackInfoResponse, ApiError> {
    let mut media_sources = media_sources(
        state,
        authenticated_user,
        target_user_id,
        item_id,
        media_source_id,
    )
    .await?;
    sort_media_sources(&mut media_sources, max_streaming_bitrate, item_id);
    Ok(PlaybackInfoResponse {
        media_sources,
        play_session_id: Uuid::new_v4().simple().to_string(),
        error_code: None,
    })
}

async fn media_source(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    item_id: Uuid,
    media_source_id: Option<&str>,
) -> Result<MediaSourceInfo, ApiError> {
    media_sources(
        state,
        authenticated_user,
        target_user_id,
        item_id,
        media_source_id,
    )
    .await?
    .into_iter()
    .next()
    .ok_or(ApiError::NotFound)
}

async fn media_sources(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    item_id: Uuid,
    media_source_id: Option<&str>,
) -> Result<Vec<MediaSourceInfo>, ApiError> {
    let item = state
        .library_controller
        .item(authenticated_user, target_user_id, item_id)
        .await?;
    let dto = user_library::project_item_to_dto(
        state,
        item,
        user_library::BaseItemDtoFields::media_sources(),
        None,
        None,
    )
    .await?;
    let mut media_sources = dto.media_sources.unwrap_or_default();
    if let Some(media_source_id) = media_source_id.filter(|value| !value.trim().is_empty()) {
        let media_source_id = media_source_id.replace('-', "");
        media_sources.retain(|source| {
            source
                .id
                .as_deref()
                .is_some_and(|source_id| source_id.replace('-', "") == media_source_id)
        });
    }
    Ok(media_sources)
}

fn sort_media_sources(
    media_sources: &mut [MediaSourceInfo],
    max_bitrate: Option<i32>,
    preferred_item_id: Uuid,
) {
    let preferred_id =
        (!preferred_item_id.is_nil()).then(|| preferred_item_id.simple().to_string());
    media_sources.sort_by_key(|source| {
        (
            preferred_rank(source, preferred_id.as_deref()),
            direct_file_rank(source),
            direct_rank(source),
            protocol_rank(source),
            bitrate_rank(source, max_bitrate),
        )
    });
}

fn preferred_rank(source: &MediaSourceInfo, preferred_id: Option<&str>) -> u8 {
    let Some(preferred_id) = preferred_id else {
        return 1;
    };
    if source.id.as_deref().is_some_and(|source_id| {
        source_id
            .chars()
            .filter(|character| *character != '-')
            .collect::<String>()
            .eq_ignore_ascii_case(preferred_id)
    }) {
        0
    } else {
        1
    }
}

fn direct_file_rank(source: &MediaSourceInfo) -> u8 {
    if source.supports_direct_play && source.protocol == MediaProtocol::File {
        0
    } else {
        1
    }
}

fn direct_rank(source: &MediaSourceInfo) -> u8 {
    if source.supports_direct_play || source.supports_direct_stream {
        0
    } else {
        1
    }
}

fn protocol_rank(source: &MediaSourceInfo) -> u8 {
    if source.protocol == MediaProtocol::File {
        0
    } else {
        1
    }
}

fn bitrate_rank(source: &MediaSourceInfo, max_bitrate: Option<i32>) -> u8 {
    match (max_bitrate, source.bitrate) {
        (Some(max_bitrate), Some(bitrate)) if bitrate <= max_bitrate => 0,
        (Some(_), Some(_)) => 2,
        _ => 1,
    }
}

fn live_stream_id(
    item_id: Uuid,
    play_session_id: Option<&str>,
    open_token: Option<&str>,
) -> String {
    let play_session_id = play_session_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default");
    let open_token = open_token
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("source");
    format!("{}:{play_session_id}:{open_token}", item_id.simple())
}

struct RepeatingChunkReader {
    remaining: u64,
    offset: usize,
}

impl RepeatingChunkReader {
    const fn new(remaining: u64) -> Self {
        Self {
            remaining,
            offset: 0,
        }
    }
}

impl AsyncRead for RepeatingChunkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.remaining == 0 || buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let remaining = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        let length = buffer
            .remaining()
            .min(REPEATING_BLOCK_SIZE - self.offset)
            .min(remaining);
        buffer.put_slice(&REPEATING_BLOCK[self.offset..self.offset + length]);
        self.remaining -= u64::try_from(length).expect("stream block length fits u64");
        self.offset = (self.offset + length) % REPEATING_BLOCK_SIZE;
        Poll::Ready(Ok(()))
    }
}

const fn bitrate_test_block() -> [u8; REPEATING_BLOCK_SIZE] {
    let mut block = [0; REPEATING_BLOCK_SIZE];
    let mut state = 0x6d2b_79f5_u32;
    let mut index = 0;
    while index < REPEATING_BLOCK_SIZE {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        block[index] = state.to_le_bytes()[0];
        index += 1;
    }
    block
}

#[cfg(test)]
mod tests {
    use super::{MediaProtocol, MediaSourceInfo, sort_media_sources};
    use uuid::Uuid;

    #[test]
    fn sort_media_sources_keeps_preferred_item_first_even_over_bitrate_limit() {
        let preferred_item_id = Uuid::new_v4();
        let preferred_source = source(preferred_item_id, 80_000_000, false);
        let sibling_source = source(Uuid::new_v4(), 8_000_000, true);
        let mut sources = vec![sibling_source, preferred_source];

        sort_media_sources(&mut sources, Some(20_000_000), preferred_item_id);

        assert_eq!(
            sources[0].id.as_deref(),
            Some(preferred_item_id.simple().to_string().as_str())
        );
    }

    #[test]
    fn sort_media_sources_without_preferred_item_orders_by_playability() {
        let direct_play = source(Uuid::new_v4(), 8_000_000, true);
        let mut transcode_only = source(Uuid::new_v4(), 8_000_000, false);
        transcode_only.supports_direct_stream = false;
        let direct_play_id = direct_play.id.clone();
        let mut sources = vec![transcode_only, direct_play];

        sort_media_sources(&mut sources, Some(20_000_000), Uuid::nil());

        assert_eq!(sources[0].id, direct_play_id);
    }

    #[test]
    fn sort_media_sources_missing_preferred_id_keeps_playability_order() {
        let direct_play = source(Uuid::new_v4(), 8_000_000, true);
        let mut transcode_only = source(Uuid::new_v4(), 8_000_000, false);
        transcode_only.supports_direct_stream = false;
        let direct_play_id = direct_play.id.clone();
        let mut sources = vec![transcode_only, direct_play];

        sort_media_sources(&mut sources, Some(20_000_000), Uuid::new_v4());

        assert_eq!(sources[0].id, direct_play_id);
    }

    fn source(item_id: Uuid, bitrate: i32, supports_direct_play: bool) -> MediaSourceInfo {
        MediaSourceInfo {
            id: Some(item_id.simple().to_string()),
            protocol: MediaProtocol::File,
            bitrate: Some(bitrate),
            supports_direct_play,
            supports_direct_stream: true,
            supports_transcoding: true,
            ..MediaSourceInfo::default()
        }
    }
}
