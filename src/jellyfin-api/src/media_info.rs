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
use jellyfin_model::{
    DeviceProfile, EncodingContext, MediaOptions, MediaProtocol, MediaSourceInfo,
    MediaStreamProtocol, MediaStreamType, PlayMethod, PlaybackErrorCode, StreamBuilder,
    SubtitleDeliveryMethod, UserPolicy,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::authentication::RemoteIp;
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
#[serde(default)]
pub(crate) struct PlaybackInfoQuery {
    #[serde(rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        rename = "maxStreamingBitrate",
        alias = "MaxStreamingBitrate",
        alias = "maxstreamingbitrate",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    max_streaming_bitrate: Option<i32>,
    #[serde(
        rename = "startTimeTicks",
        alias = "StartTimeTicks",
        alias = "starttimeticks",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    start_time_ticks: Option<i64>,
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "subtitleStreamIndex",
        alias = "SubtitleStreamIndex",
        alias = "subtitlestreamindex",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    subtitle_stream_index: Option<i32>,
    #[serde(
        rename = "maxAudioChannels",
        alias = "MaxAudioChannels",
        alias = "maxaudiochannels",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    max_audio_channels: Option<i32>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(
        rename = "liveStreamId",
        alias = "LiveStreamId",
        alias = "livestreamid"
    )]
    live_stream_id: Option<String>,
    #[serde(
        rename = "autoOpenLiveStream",
        alias = "AutoOpenLiveStream",
        alias = "autoopenlivestream",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    auto_open_live_stream: Option<bool>,
    #[serde(
        rename = "enableDirectPlay",
        alias = "EnableDirectPlay",
        alias = "enabledirectplay",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    enable_direct_play: Option<bool>,
    #[serde(
        rename = "enableDirectStream",
        alias = "EnableDirectStream",
        alias = "enabledirectstream",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    enable_direct_stream: Option<bool>,
    #[serde(
        rename = "enableTranscoding",
        alias = "EnableTranscoding",
        alias = "enabletranscoding",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    enable_transcoding: Option<bool>,
    #[serde(
        rename = "allowVideoStreamCopy",
        alias = "AllowVideoStreamCopy",
        alias = "allowvideostreamcopy",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    allow_video_stream_copy: Option<bool>,
    #[serde(
        rename = "allowAudioStreamCopy",
        alias = "AllowAudioStreamCopy",
        alias = "allowaudiostreamcopy",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    allow_audio_stream_copy: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct PlaybackInfoDto {
    #[serde(alias = "userId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        alias = "maxStreamingBitrate",
        alias = "maxstreamingbitrate",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    max_streaming_bitrate: Option<i32>,
    #[serde(
        alias = "startTimeTicks",
        alias = "starttimeticks",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    start_time_ticks: Option<i64>,
    #[serde(
        alias = "audioStreamIndex",
        alias = "audiostreamindex",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        alias = "subtitleStreamIndex",
        alias = "subtitlestreamindex",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    subtitle_stream_index: Option<i32>,
    #[serde(
        alias = "maxAudioChannels",
        alias = "maxaudiochannels",
        deserialize_with = "deserialize_optional_number_or_string"
    )]
    max_audio_channels: Option<i32>,
    #[serde(alias = "mediaSourceId", alias = "mediasourceid")]
    media_source_id: Option<String>,
    #[serde(alias = "liveStreamId", alias = "livestreamid")]
    live_stream_id: Option<String>,
    #[serde(alias = "deviceProfile", alias = "deviceprofile")]
    device_profile: Option<Value>,
    #[serde(
        alias = "enableDirectPlay",
        alias = "enabledirectplay",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    enable_direct_play: Option<bool>,
    #[serde(
        alias = "enableDirectStream",
        alias = "enabledirectstream",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    enable_direct_stream: Option<bool>,
    #[serde(
        alias = "enableTranscoding",
        alias = "enabletranscoding",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    enable_transcoding: Option<bool>,
    #[serde(
        alias = "allowVideoStreamCopy",
        alias = "allowvideostreamcopy",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    allow_video_stream_copy: Option<bool>,
    #[serde(
        alias = "allowAudioStreamCopy",
        alias = "allowaudiostreamcopy",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    allow_audio_stream_copy: Option<bool>,
    #[serde(
        alias = "autoOpenLiveStream",
        alias = "autoopenlivestream",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    auto_open_live_stream: Option<bool>,
    #[serde(
        alias = "alwaysBurnInSubtitleWhenTranscoding",
        alias = "alwaysburninsubtitlewhentranscoding",
        deserialize_with = "deserialize_optional_bool_or_string"
    )]
    always_burn_in_subtitle_when_transcoding: Option<bool>,
}

#[derive(Debug)]
struct PlaybackOptions {
    media_source_id: Option<String>,
    max_streaming_bitrate: Option<i32>,
    start_time_ticks: i64,
    audio_stream_index: Option<i32>,
    subtitle_stream_index: Option<i32>,
    max_audio_channels: Option<i32>,
    device_profile: Option<DeviceProfile>,
    enable_direct_play: bool,
    enable_direct_stream: bool,
    enable_transcoding: bool,
    allow_video_stream_copy: bool,
    allow_audio_stream_copy: bool,
    always_burn_in_subtitle_when_transcoding: bool,
}

impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            media_source_id: None,
            max_streaming_bitrate: None,
            start_time_ticks: 0,
            audio_stream_index: None,
            subtitle_stream_index: None,
            max_audio_channels: None,
            device_profile: None,
            enable_direct_play: true,
            enable_direct_stream: true,
            enable_transcoding: true,
            allow_video_stream_copy: true,
            allow_audio_stream_copy: true,
            always_burn_in_subtitle_when_transcoding: false,
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    play_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<PlaybackErrorCode>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct LiveStreamResponse {
    media_source: MediaSourceInfo,
}

fn deserialize_optional_number_or_string<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned + std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => value.parse().map(Some).map_err(serde::de::Error::custom),
        Some(value) => serde_json::from_value(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn deserialize_optional_bool_or_string<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) if value.eq_ignore_ascii_case("true") => Ok(Some(true)),
        Some(Value::String(value)) if value.eq_ignore_ascii_case("false") => Ok(Some(false)),
        Some(Value::String(value)) => Err(serde::de::Error::custom(format_args!(
            "invalid boolean `{value}`"
        ))),
        Some(value) => serde_json::from_value(value)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn stored_device_profile(capabilities: &Value) -> Option<DeviceProfile> {
    let profile = capabilities
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("DeviceProfile"))?
        .1
        .clone();
    match parse_device_profile(profile) {
        Ok(profile) => Some(profile),
        Err(error) => {
            tracing::warn!(%error, "ignoring invalid stored device profile");
            None
        }
    }
}

fn parse_device_profile(mut value: Value) -> Result<DeviceProfile, serde_json::Error> {
    normalize_device_profile_json(&mut value, None);
    serde_json::from_value(value)
}

fn normalize_device_profile_json(value: &mut Value, context: Option<&str>) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_device_profile_json(value, context);
            }
        }
        Value::Object(object) => {
            let original = std::mem::take(object);
            for (name, mut child) in original {
                let name = canonical_device_profile_key(&name)
                    .map(str::to_owned)
                    .unwrap_or(name);
                normalize_device_profile_scalar(&name, context, &mut child);
                normalize_device_profile_json(&mut child, Some(&name));
                object.insert(name, child);
            }
        }
        _ => {}
    }
}

fn normalize_device_profile_scalar(name: &str, context: Option<&str>, value: &mut Value) {
    match name {
        "MaxStreamingBitrate"
        | "MaxStaticBitrate"
        | "MusicStreamingTranscodingBitrate"
        | "MaxStaticMusicBitrate"
        | "MinSegments"
        | "SegmentLength" => normalize_json_number(value),
        "Type" if context == Some("CodecProfiles") => {
            normalize_json_enum(value, &[(0, "Video"), (1, "VideoAudio"), (2, "Audio")])
        }
        "Type" => normalize_json_enum(
            value,
            &[
                (0, "Audio"),
                (1, "Video"),
                (2, "Photo"),
                (3, "Subtitle"),
                (4, "Lyric"),
            ],
        ),
        "Protocol" => normalize_json_enum(value, &[(0, "http"), (1, "hls")]),
        "Context" => normalize_json_enum(value, &[(0, "Streaming"), (1, "Static")]),
        "TranscodeSeekInfo" => normalize_json_enum(value, &[(0, "Auto"), (1, "Bytes")]),
        "Method" => normalize_json_enum(
            value,
            &[
                (0, "Encode"),
                (1, "Embed"),
                (2, "External"),
                (3, "Hls"),
                (4, "Drop"),
            ],
        ),
        "Condition" => normalize_json_enum(
            value,
            &[
                (0, "Equals"),
                (1, "NotEquals"),
                (2, "LessThanEqual"),
                (3, "GreaterThanEqual"),
                (4, "EqualsAny"),
            ],
        ),
        "Property" => normalize_json_enum(value, PROFILE_CONDITION_VALUES),
        _ => {}
    }
}

const PROFILE_CONDITION_VALUES: &[(i64, &str)] = &[
    (0, "AudioChannels"),
    (1, "AudioBitrate"),
    (2, "AudioProfile"),
    (3, "Width"),
    (4, "Height"),
    (5, "Has64BitOffsets"),
    (6, "PacketLength"),
    (7, "VideoBitDepth"),
    (8, "VideoBitrate"),
    (9, "VideoFramerate"),
    (10, "VideoLevel"),
    (11, "VideoProfile"),
    (12, "VideoTimestamp"),
    (13, "IsAnamorphic"),
    (14, "RefFrames"),
    (16, "NumAudioStreams"),
    (17, "NumVideoStreams"),
    (18, "IsSecondaryAudio"),
    (19, "VideoCodecTag"),
    (20, "IsAvc"),
    (21, "IsInterlaced"),
    (22, "AudioSampleRate"),
    (23, "AudioBitDepth"),
    (24, "VideoRangeType"),
    (25, "NumStreams"),
    (26, "VideoRotation"),
];

fn normalize_json_number(value: &mut Value) {
    if let Value::String(text) = value
        && let Ok(number) = text.parse::<i64>()
    {
        *value = Value::Number(number.into());
    }
}

fn normalize_json_enum(value: &mut Value, variants: &[(i64, &str)]) {
    let number = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.parse::<i64>().ok(),
        _ => None,
    };
    let variant = number
        .and_then(|number| {
            variants
                .iter()
                .find(|(value, _)| *value == number)
                .map(|(_, name)| *name)
        })
        .or_else(|| {
            value.as_str().and_then(|text| {
                variants
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(text))
                    .map(|(_, name)| *name)
            })
        });
    if let Some(variant) = variant {
        *value = Value::String(variant.to_owned());
    }
}

fn canonical_device_profile_key(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "name" => "Name",
        "id" => "Id",
        "maxstreamingbitrate" => "MaxStreamingBitrate",
        "maxstaticbitrate" => "MaxStaticBitrate",
        "musicstreamingtranscodingbitrate" => "MusicStreamingTranscodingBitrate",
        "maxstaticmusicbitrate" => "MaxStaticMusicBitrate",
        "directplayprofiles" => "DirectPlayProfiles",
        "transcodingprofiles" => "TranscodingProfiles",
        "containerprofiles" => "ContainerProfiles",
        "codecprofiles" => "CodecProfiles",
        "subtitleprofiles" => "SubtitleProfiles",
        "container" => "Container",
        "audiocodec" => "AudioCodec",
        "videocodec" => "VideoCodec",
        "type" => "Type",
        "protocol" => "Protocol",
        "estimatecontentlength" => "EstimateContentLength",
        "enablempegtsm2tsmode" => "EnableMpegtsM2TsMode",
        "transcodeseekinfo" => "TranscodeSeekInfo",
        "copytimestamps" => "CopyTimestamps",
        "context" => "Context",
        "enablesubtitlesinmanifest" => "EnableSubtitlesInManifest",
        "maxaudiochannels" => "MaxAudioChannels",
        "minsegments" => "MinSegments",
        "segmentlength" => "SegmentLength",
        "conditions" => "Conditions",
        "enableaudiovbrencoding" => "EnableAudioVbrEncoding",
        "applyconditions" => "ApplyConditions",
        "codec" => "Codec",
        "subcontainer" => "SubContainer",
        "format" => "Format",
        "method" => "Method",
        "language" => "Language",
        "condition" => "Condition",
        "property" => "Property",
        "value" => "Value",
        "isrequired" => "IsRequired",
        _ => return None,
    })
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
    RemoteIp(remote_ip): RemoteIp,
    headers: axum::http::HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<PlaybackInfoQuery>, QueryRejection>,
) -> Result<Json<PlaybackInfoResponse>, ApiError> {
    let identity = authentication::authenticated_session(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let target_user_id = query.user_id.unwrap_or(identity.user.id);
    let _live_stream_id = query.live_stream_id;
    let _auto_open_live_stream = query.auto_open_live_stream.unwrap_or_default();
    let options = PlaybackOptions {
        media_source_id: query.media_source_id,
        max_streaming_bitrate: query.max_streaming_bitrate,
        start_time_ticks: query.start_time_ticks.unwrap_or_default(),
        audio_stream_index: query.audio_stream_index,
        subtitle_stream_index: query.subtitle_stream_index,
        max_audio_channels: query.max_audio_channels,
        enable_direct_play: query.enable_direct_play.unwrap_or(true),
        enable_direct_stream: query.enable_direct_stream.unwrap_or(true),
        enable_transcoding: query.enable_transcoding.unwrap_or(true),
        allow_video_stream_copy: query.allow_video_stream_copy.unwrap_or(true),
        allow_audio_stream_copy: query.allow_audio_stream_copy.unwrap_or(true),
        ..PlaybackOptions::default()
    };
    playback_info(
        &state,
        &identity.user,
        target_user_id,
        item_id,
        options,
        &identity.device.device_id,
        &identity.access_token,
        remote_ip,
    )
    .await
    .map(Json)
}

pub(crate) async fn post_playback_info(
    State(state): State<Arc<AppState>>,
    RemoteIp(remote_ip): RemoteIp,
    headers: axum::http::HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<PlaybackInfoQuery>, QueryRejection>,
    body: Result<Option<Json<PlaybackInfoDto>>, JsonRejection>,
) -> Result<Json<PlaybackInfoResponse>, ApiError> {
    let identity = authentication::authenticated_session(&state, &headers).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let body = optional_playback_body(body)?.unwrap_or_default();
    let target_user_id = query.user_id.or(body.user_id).unwrap_or(identity.user.id);
    let device_profile = match body.device_profile {
        Some(profile) => Some(parse_device_profile(profile).map_err(|error| {
            tracing::debug!(%error, "invalid playback device profile");
            ApiError::InvalidRequest
        })?),
        None => stored_device_profile(&identity.device.capabilities),
    };
    // These legacy parameters are relevant only to live sources. Parse and
    // merge them for wire compatibility, but do not synthesize Live TV state
    // for ordinary file playback.
    let _live_stream_id = query.live_stream_id.or(body.live_stream_id);
    let _auto_open_live_stream = query
        .auto_open_live_stream
        .or(body.auto_open_live_stream)
        .unwrap_or_default();
    let options = PlaybackOptions {
        media_source_id: query.media_source_id.or(body.media_source_id),
        max_streaming_bitrate: query.max_streaming_bitrate.or(body.max_streaming_bitrate),
        start_time_ticks: query
            .start_time_ticks
            .or(body.start_time_ticks)
            .unwrap_or_default(),
        audio_stream_index: query.audio_stream_index.or(body.audio_stream_index),
        subtitle_stream_index: query.subtitle_stream_index.or(body.subtitle_stream_index),
        max_audio_channels: query.max_audio_channels.or(body.max_audio_channels),
        device_profile,
        enable_direct_play: query
            .enable_direct_play
            .or(body.enable_direct_play)
            .unwrap_or(true),
        enable_direct_stream: query
            .enable_direct_stream
            .or(body.enable_direct_stream)
            .unwrap_or(true),
        enable_transcoding: query
            .enable_transcoding
            .or(body.enable_transcoding)
            .unwrap_or(true),
        allow_video_stream_copy: query
            .allow_video_stream_copy
            .or(body.allow_video_stream_copy)
            .unwrap_or(true),
        allow_audio_stream_copy: query
            .allow_audio_stream_copy
            .or(body.allow_audio_stream_copy)
            .unwrap_or(true),
        always_burn_in_subtitle_when_transcoding: body
            .always_burn_in_subtitle_when_transcoding
            .unwrap_or_default(),
    };
    playback_info(
        &state,
        &identity.user,
        target_user_id,
        item_id,
        options,
        &identity.device.device_id,
        &identity.access_token,
        remote_ip,
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
        Ok(None) | Err(JsonRejection::MissingJsonContentType(_)) => Ok(None),
        Err(_) => Err(ApiError::InvalidRequest),
    }
}

fn optional_open_live_stream_body(
    body: Result<Option<Json<OpenLiveStreamDto>>, JsonRejection>,
) -> Result<Option<OpenLiveStreamDto>, ApiError> {
    match body {
        Ok(Some(Json(body))) => Ok(Some(body)),
        Ok(None) | Err(JsonRejection::MissingJsonContentType(_)) => Ok(None),
        Err(_) => Err(ApiError::InvalidRequest),
    }
}

#[allow(clippy::too_many_arguments)]
async fn playback_info(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    item_id: Uuid,
    options: PlaybackOptions,
    device_id: &str,
    access_token: &str,
    remote_ip: std::net::IpAddr,
) -> Result<PlaybackInfoResponse, ApiError> {
    let mut max_streaming_bitrate = options.max_streaming_bitrate;
    let has_device_profile = options.device_profile.is_some();
    let mut media_sources = media_sources(
        state,
        authenticated_user,
        target_user_id,
        item_id,
        options.media_source_id.as_deref(),
    )
    .await?;
    if media_sources.is_empty() {
        tracing::warn!(%item_id, %device_id, has_device_profile, "no compatible media source found");
        return Ok(PlaybackInfoResponse {
            media_sources,
            play_session_id: None,
            error_code: Some(PlaybackErrorCode::NoCompatibleStream),
        });
    }
    let target_user_policy =
        user_library::media_source_policy_for_user(state, target_user_id).await?;
    let play_session_id = Uuid::new_v4().simple().to_string();
    apply_stream_builder(
        &mut media_sources,
        &target_user_policy,
        state,
        item_id,
        &options,
        &mut max_streaming_bitrate,
        device_id,
        access_token,
        &play_session_id,
        remote_ip,
    );
    if let Some(source) = media_sources.first() {
        tracing::info!(
            %item_id,
            %device_id,
            %play_session_id,
            has_device_profile,
            source_count = media_sources.len(),
            media_source_id = source.id.as_deref().unwrap_or_default(),
            protocol = ?source.protocol,
            container = source.container.as_deref().unwrap_or_default(),
            bitrate = ?source.bitrate,
            streams = source.media_streams.len(),
            supports_direct_play = source.supports_direct_play,
            supports_direct_stream = source.supports_direct_stream,
            supports_transcoding = source.supports_transcoding,
            selected_transcoding = source.transcoding_url.is_some(),
            "playback information prepared",
        );
    } else {
        tracing::warn!(%item_id, %device_id, has_device_profile, "no playable media source found");
    }
    Ok(PlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
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
    if let Some(source_id) = selected_playback_source_id(state, item_id, media_source_id).await? {
        state
            .library_scan
            .hydrate_strm_media_streams(source_id)
            .await?;
    }
    let dto = user_library::project_item_to_dto(
        state,
        item,
        target_user_id,
        user_library::BaseItemDtoFields::media_sources(),
        None,
        None,
    )
    .await?;
    let mut media_sources = dto.media_sources.unwrap_or_default();
    if let Some(media_source_id) = media_source_id.filter(|value| !value.trim().is_empty()) {
        let media_source_id = media_source_id.replace('-', "");
        media_sources.retain(|source| {
            source.id.as_deref().is_some_and(|source_id| {
                source_id
                    .replace('-', "")
                    .eq_ignore_ascii_case(&media_source_id)
            })
        });
    }
    Ok(media_sources)
}

async fn selected_playback_source_id(
    state: &AppState,
    item_id: Uuid,
    media_source_id: Option<&str>,
) -> Result<Option<Uuid>, ApiError> {
    let Some(media_source_id) = media_source_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(Some(item_id));
    };
    let Ok(source_id) = Uuid::parse_str(media_source_id) else {
        return Ok(None);
    };
    if source_id == item_id {
        return Ok(Some(item_id));
    }
    Ok(state
        .base_items
        .alternate_video_version(item_id, source_id)
        .await?
        .map(|source| source.id))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn apply_stream_builder(
    media_sources: &mut Vec<MediaSourceInfo>,
    policy: &UserPolicy,
    state: &AppState,
    item_id: Uuid,
    playback_options: &PlaybackOptions,
    max_streaming_bitrate: &mut Option<i32>,
    device_id: &str,
    access_token: &str,
    play_session_id: &str,
    remote_ip: std::net::IpAddr,
) {
    let Some(profile) = playback_options.device_profile.clone() else {
        for source in media_sources {
            apply_unprofiled_playback_capabilities(source);
        }
        return;
    };
    let remote_client_bitrate_limit = policy.remote_client_bitrate_limit;
    if !state.network_manager.is_in_local_network(remote_ip) && remote_client_bitrate_limit > 0 {
        *max_streaming_bitrate = Some(
            max_streaming_bitrate.map_or(remote_client_bitrate_limit, |bitrate| {
                bitrate.min(remote_client_bitrate_limit)
            }),
        );
    }
    let builder =
        StreamBuilder::with_encodable_audio_codecs(["aac", "mp3", "opus", "flac", "ac3", "eac3"]);
    let requested_source_id = playback_options
        .media_source_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.replace('-', ""));
    let original_sources = std::mem::take(media_sources);
    let mut projected_sources = Vec::with_capacity(original_sources.len());
    for mut source in original_sources {
        let is_video = source.video_type.is_some() || source.video_stream().is_some();
        let source_is_selected = requested_source_id.as_deref().is_some_and(|requested| {
            source
                .id
                .as_deref()
                .is_some_and(|source_id| source_id.replace('-', "").eq_ignore_ascii_case(requested))
        });
        let selected_source_id = source_is_selected.then(|| source.id.clone()).flatten();
        let can_transcode = playback_options.enable_transcoding
            && policy_can_transcode(policy, !is_video)
            && source.path.as_deref().is_some_and(|path| !path.is_empty());
        source.supports_direct_play &= playback_options.enable_direct_play
            && source.path.as_deref().is_some_and(|path| !path.is_empty())
            && (source.protocol == MediaProtocol::File
                || (is_video && source.protocol == MediaProtocol::Http));
        // Ordinary HTTP direct-stream/remux URLs are not implemented: the
        // static route streams the original bytes regardless of its suffix.
        source.supports_direct_stream = false;
        source.supports_transcoding &= can_transcode;
        let mut options = MediaOptions {
            enable_transcoding: can_transcode,
            enable_direct_play: playback_options.enable_direct_play,
            enable_direct_stream: playback_options.enable_direct_stream
                && source.supports_direct_stream,
            enable_playback_remuxing: policy.enable_playback_remuxing,
            force_remote_source_transcoding: policy.force_remote_source_transcoding,
            allow_audio_stream_copy: playback_options.allow_audio_stream_copy,
            allow_video_stream_copy: playback_options.allow_video_stream_copy,
            always_burn_in_subtitle_when_transcoding: playback_options
                .always_burn_in_subtitle_when_transcoding,
            item_id,
            media_sources: vec![source],
            profile: profile.clone(),
            media_source_id: selected_source_id,
            device_id: Some(device_id.to_owned()),
            max_bitrate: *max_streaming_bitrate,
            audio_transcoding_bitrate: *max_streaming_bitrate,
            audio_stream_index: source_is_selected
                .then_some(playback_options.audio_stream_index)
                .flatten(),
            subtitle_stream_index: source_is_selected
                .then_some(playback_options.subtitle_stream_index)
                .flatten(),
            max_audio_channels: playback_options.max_audio_channels,
            context: EncodingContext::Streaming,
            ..MediaOptions::default()
        };
        let selection = if is_video {
            builder.take_optimal_video_stream(&mut options)
        } else {
            builder.take_optimal_audio_stream(&mut options)
        };
        let Ok(Some((_source_index, mut stream))) = selection else {
            tracing::warn!(
                %item_id,
                %device_id,
                is_video,
                media_source_id = options
                    .media_sources
                    .first()
                    .and_then(|source| source.id.as_deref())
                    .unwrap_or_default(),
                "device profile did not produce a playable stream",
            );
            for source in &mut options.media_sources {
                clear_playback_capabilities(source);
            }
            projected_sources.append(&mut options.media_sources);
            continue;
        };
        tracing::info!(
            %item_id,
            %device_id,
            media_source_id = stream.media_source_id().unwrap_or_default(),
            play_method = ?stream.play_method,
            container = stream.container.as_deref().unwrap_or_default(),
            video_codecs = ?stream.video_codecs,
            audio_codecs = ?stream.audio_codecs,
            video_bitrate = ?stream.video_bitrate,
            audio_bitrate = ?stream.audio_bitrate,
            segment_length = ?stream.segment_length,
            transcode_reason_bits = stream.transcode_reasons.bits(),
            "playback stream selected from device profile",
        );
        stream.play_session_id = Some(play_session_id.to_owned());
        stream.start_position_ticks = playback_options.start_time_ticks;
        apply_selected_stream_metadata(&mut stream, &options, access_token, &builder);
        projected_sources.push(
            stream
                .media_source
                .take()
                .expect("selected stream always owns its media source"),
        );
    }
    *media_sources = projected_sources;
}

fn apply_selected_stream_metadata(
    stream: &mut jellyfin_model::StreamInfo,
    options: &MediaOptions,
    access_token: &str,
    builder: &StreamBuilder,
) {
    let play_method = stream.play_method;
    let supports_direct_play = play_method == PlayMethod::DirectPlay
        && options.enable_direct_play
        && stream
            .media_source
            .as_ref()
            .is_some_and(|source| source.supports_direct_play);
    let transcoding = if play_method == PlayMethod::Transcode
        && options.enable_transcoding
        && stream.sub_protocol == MediaStreamProtocol::Hls
    {
        // Clients already know the externally reachable server URL. Returning
        // a relative path avoids leaking an internal bind address such as
        // `http://0.0.0.0:8096` when the server runs behind Docker or a proxy.
        let url = stream.to_url(None, Some(access_token), None);
        (!url.is_empty()).then(|| (url, stream.container.clone(), stream.sub_protocol))
    } else {
        None
    };
    if let Some(source) = stream.media_source.as_mut() {
        clear_playback_capabilities(source);
        source.supports_direct_play = supports_direct_play;
        source.supports_transcoding = transcoding.is_some();
        source.default_audio_stream_index = stream.audio_stream_index;
        source.default_subtitle_stream_index = stream.subtitle_stream_index;
        for subtitle in source
            .media_streams
            .iter_mut()
            .filter(|media_stream| media_stream.stream_type == MediaStreamType::Subtitle)
        {
            let (delivery_method, delivery_format) = {
                let profile = builder.get_subtitle_profile(
                    subtitle,
                    &options.profile.subtitle_profiles,
                    play_method,
                    stream.container.as_deref(),
                    Some(stream.sub_protocol),
                );
                (profile.method, profile.format.to_owned())
            };
            subtitle.delivery_method = Some(delivery_method);
            subtitle.delivery_url = None;
            subtitle.is_external_url = None;
            if delivery_method == SubtitleDeliveryMethod::External {
                let source_id = source.id.as_deref().unwrap_or_default();
                let mut delivery_url = format!(
                    "/Videos/{}/{}/Subtitles/{}/{}/Stream.{}",
                    stream.item_id,
                    source_id,
                    subtitle.index,
                    stream.start_position_ticks,
                    delivery_format,
                );
                let is_direct_external_url = subtitle.is_external
                    && subtitle.supports_external_stream
                    && subtitle
                        .codec
                        .as_deref()
                        .is_some_and(|codec| codec.eq_ignore_ascii_case(&delivery_format))
                    && subtitle.path.as_deref().is_some_and(|path| {
                        path.starts_with("http://") || path.starts_with("https://")
                    });
                if is_direct_external_url {
                    delivery_url = subtitle.path.clone().unwrap_or_default();
                    subtitle.is_external_url = Some(true);
                } else {
                    delivery_url.push_str("?ApiKey=");
                    delivery_url.push_str(access_token);
                    subtitle.is_external_url = Some(false);
                }
                subtitle.delivery_url = Some(delivery_url);
            }
        }
        for attachment in &mut source.media_attachments {
            attachment.delivery_url = Some(format!(
                "/Videos/{}/{}/Attachments/{}",
                stream.item_id,
                source.id.as_deref().unwrap_or_default(),
                attachment.index
            ));
        }
        if let Some((url, container, sub_protocol)) = transcoding {
            source.transcoding_url = Some(url);
            source.transcoding_container = container;
            source.transcoding_sub_protocol = sub_protocol;
        }
    }
}

fn clear_playback_capabilities(source: &mut MediaSourceInfo) {
    source.supports_direct_play = false;
    source.supports_direct_stream = false;
    source.supports_transcoding = false;
    source.transcoding_url = None;
    source.transcoding_container = None;
    source.transcoding_sub_protocol = MediaStreamProtocol::Http;
}

/// Match the official PlaybackInfo behavior when no device profile is
/// available. The server cannot select a transcode profile in that case, but
/// it can still expose static playback when the corresponding route really
/// serves the source bytes. In particular, do not turn a valid local file
/// into a source with no playable method just because the client omitted its
/// profile.
fn apply_unprofiled_playback_capabilities(source: &mut MediaSourceInfo) {
    let is_video = source.video_type.is_some() || source.video_stream().is_some();
    source.supports_direct_play = source.path.as_deref().is_some_and(|path| !path.is_empty())
        && (source.protocol == MediaProtocol::File
            || (is_video && source.protocol == MediaProtocol::Http));
    source.supports_direct_stream = false;
    source.supports_transcoding = false;
    source.transcoding_url = None;
    source.transcoding_container = None;
    source.transcoding_sub_protocol = MediaStreamProtocol::Http;
}

const fn policy_can_transcode(policy: &jellyfin_model::UserPolicy, is_audio: bool) -> bool {
    if is_audio {
        policy.enable_audio_playback_transcoding
    } else {
        policy.enable_audio_playback_transcoding
            || policy.enable_video_playback_transcoding
            || policy.enable_playback_remuxing
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
    use std::sync::Arc;

    use crate::AppState;

    use jellyfin_model::{
        DeviceProfile, DirectPlayProfile, DlnaProfileType, EncodingContext, MediaAttachment,
        MediaOptions, MediaStream, MediaStreamProtocol, MediaStreamType, PlayMethod, StreamBuilder,
        SubtitleDeliveryMethod, SubtitleProfile, TranscodingProfile, UserPolicy,
    };

    use super::{
        MediaProtocol, MediaSourceInfo, PlaybackOptions, apply_selected_stream_metadata,
        apply_stream_builder,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn playback_info_without_profile_keeps_static_playback_available() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let mut sources = vec![
            source(first_id, 8_000_000, true),
            source(second_id, 4_000_000, true),
        ];

        apply_stream_builder(
            &mut sources,
            &UserPolicy::default(),
            &test_state(),
            first_id,
            &PlaybackOptions::default(),
            &mut None,
            "device-id",
            "access-token",
            "play-session-id",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );

        assert_eq!(
            sources
                .iter()
                .map(|source| source.id.clone())
                .collect::<Vec<_>>(),
            [
                Some(first_id.simple().to_string()),
                Some(second_id.simple().to_string())
            ]
        );
        assert!(sources.iter().all(|source| {
            source.supports_direct_play
                && !source.supports_direct_stream
                && !source.supports_transcoding
                && source.transcoding_url.is_none()
        }));
    }

    #[tokio::test]
    async fn playback_info_uses_client_profile_and_exposes_relative_transcoding_url() {
        let item_id = Uuid::new_v4();
        let source = MediaSourceInfo {
            id: Some(item_id.simple().to_string()),
            protocol: MediaProtocol::File,
            path: Some("/media/movie.mkv".to_owned()),
            container: Some("mkv".to_owned()),
            run_time_ticks: Some(600_000_000),
            media_streams: vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    width: Some(1920),
                    height: Some(1080),
                    is_default: true,
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("aac".to_owned()),
                    channels: Some(2),
                    is_default: true,
                    ..MediaStream::default()
                },
            ],
            ..MediaSourceInfo::default()
        };
        let profile = DeviceProfile {
            max_streaming_bitrate: Some(8_000_000),
            direct_play_profiles: Vec::new(),
            transcoding_profiles: vec![TranscodingProfile {
                container: "ts".to_owned(),
                profile_type: DlnaProfileType::Video,
                video_codec: "h264".to_owned(),
                audio_codec: "aac".to_owned(),
                protocol: MediaStreamProtocol::Hls,
                context: EncodingContext::Streaming,
                segment_length: 6,
                ..TranscodingProfile::default()
            }],
            ..DeviceProfile::default()
        };
        let mut sources = vec![source];

        apply_stream_builder(
            &mut sources,
            &UserPolicy::default(),
            &test_state(),
            item_id,
            &PlaybackOptions {
                device_profile: Some(profile),
                ..PlaybackOptions::default()
            },
            &mut None,
            "device-id",
            "access-token",
            "play-session-id",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );

        let url = sources[0]
            .transcoding_url
            .as_deref()
            .expect("transcoding url");
        assert!(url.starts_with("/videos/"));
        assert!(url.contains("/master.m3u8"));
        assert!(url.contains("DeviceId=device-id"));
        assert!(url.contains("MediaSourceId="));
        assert!(url.contains("ApiKey=access-token"));
        assert!(!sources[0].supports_direct_play);
        assert!(!sources[0].supports_direct_stream);
        assert!(sources[0].supports_transcoding);
        assert_eq!(sources[0].transcoding_container.as_deref(), Some("ts"));
        assert_eq!(
            sources[0].transcoding_sub_protocol,
            MediaStreamProtocol::Hls
        );
    }

    #[tokio::test]
    async fn playback_info_does_not_wrap_mkv_as_static_mp4_direct_stream() {
        let item_id = Uuid::new_v4();
        let source = MediaSourceInfo {
            id: Some(item_id.simple().to_string()),
            protocol: MediaProtocol::File,
            path: Some("/media/movie.mkv".to_owned()),
            container: Some("mkv".to_owned()),
            run_time_ticks: Some(600_000_000),
            media_streams: vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    width: Some(1920),
                    height: Some(1080),
                    is_default: true,
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("aac".to_owned()),
                    channels: Some(2),
                    is_default: true,
                    ..MediaStream::default()
                },
            ],
            ..MediaSourceInfo::default()
        };
        let profile = DeviceProfile {
            direct_play_profiles: vec![DirectPlayProfile {
                container: "mp4".to_owned(),
                audio_codec: Some("aac".to_owned()),
                video_codec: Some("h264".to_owned()),
                profile_type: DlnaProfileType::Video,
            }],
            transcoding_profiles: vec![TranscodingProfile {
                container: "ts".to_owned(),
                profile_type: DlnaProfileType::Video,
                video_codec: "h264".to_owned(),
                audio_codec: "aac".to_owned(),
                protocol: MediaStreamProtocol::Hls,
                context: EncodingContext::Streaming,
                segment_length: 6,
                ..TranscodingProfile::default()
            }],
            ..DeviceProfile::default()
        };
        let mut sources = vec![source];

        apply_stream_builder(
            &mut sources,
            &UserPolicy::default(),
            &test_state(),
            item_id,
            &PlaybackOptions {
                device_profile: Some(profile),
                ..PlaybackOptions::default()
            },
            &mut None,
            "device-id",
            "access-token",
            "play-session-id",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );

        let url = sources[0]
            .transcoding_url
            .as_deref()
            .expect("transcoding url");
        assert!(url.contains("/master.m3u8"));
        assert!(!url.contains("/stream.mp4"));
        assert!(!url.contains("Static=true"));
        assert!(!sources[0].supports_direct_play);
        assert!(!sources[0].supports_direct_stream);
        assert!(sources[0].supports_transcoding);
        assert_eq!(sources[0].transcoding_container.as_deref(), Some("ts"));
        assert_eq!(
            sources[0].transcoding_sub_protocol,
            MediaStreamProtocol::Hls
        );
    }

    #[test]
    fn selected_playback_stream_exposes_android_subtitle_and_attachment_urls() {
        let item_id = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        let mut stream = jellyfin_model::StreamInfo::default();
        stream.item_id = item_id;
        stream.play_method = PlayMethod::DirectPlay;
        stream.container = Some("mkv".to_owned());
        stream.sub_protocol = MediaStreamProtocol::Http;
        stream.start_position_ticks = 123;
        stream.media_source = Some(MediaSourceInfo {
            id: Some(source_id.simple().to_string()),
            media_streams: vec![MediaStream {
                index: 2,
                stream_type: MediaStreamType::Subtitle,
                codec: Some("srt".to_owned()),
                path: Some("/media/movie.srt".to_owned()),
                supports_external_stream: true,
                ..MediaStream::default()
            }],
            media_attachments: vec![MediaAttachment {
                index: 4,
                ..MediaAttachment::default()
            }],
            supports_direct_play: true,
            ..MediaSourceInfo::default()
        });
        let options = MediaOptions {
            profile: DeviceProfile {
                subtitle_profiles: vec![SubtitleProfile {
                    format: "srt".to_owned(),
                    method: SubtitleDeliveryMethod::External,
                    ..SubtitleProfile::default()
                }],
                ..DeviceProfile::default()
            },
            enable_direct_play: true,
            ..MediaOptions::default()
        };

        let builder = StreamBuilder::default();
        apply_selected_stream_metadata(&mut stream, &options, "token", &builder);

        let source = stream.media_source.expect("selected source");
        let subtitle = &source.media_streams[0];
        assert_eq!(
            subtitle.delivery_method,
            Some(SubtitleDeliveryMethod::External)
        );
        assert_eq!(subtitle.is_external_url, Some(false));
        assert_eq!(
            subtitle.delivery_url.as_deref(),
            Some(format!(
                "/Videos/{}/{}/Subtitles/2/123/Stream.srt?ApiKey=token",
                item_id,
                source_id.simple()
            ))
            .as_deref()
        );
        assert_eq!(
            source.media_attachments[0].delivery_url.as_deref(),
            Some(format!(
                "/Videos/{}/{}/Attachments/4",
                item_id,
                source_id.simple()
            ))
            .as_deref()
        );
    }

    fn source(item_id: Uuid, bitrate: i32, supports_direct_play: bool) -> MediaSourceInfo {
        MediaSourceInfo {
            id: Some(item_id.simple().to_string()),
            protocol: MediaProtocol::File,
            path: Some(format!("/media/{item_id}.mkv")),
            bitrate: Some(bitrate),
            supports_direct_play,
            supports_direct_stream: true,
            supports_transcoding: true,
            ..MediaSourceInfo::default()
        }
    }

    fn test_state() -> Arc<AppState> {
        let database = sea_orm::DatabaseConnection::Disconnected;
        AppState::new(
            database,
            "Playback Test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .into()
    }
}
