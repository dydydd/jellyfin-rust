use std::{io, path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, Request},
    response::{IntoResponse, Redirect, Response},
};
use bytes::Bytes;
use futures_util::stream;
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use jellyfin_controller::transcode::TranscodeJobHandle;
use jellyfin_controller::{FfmpegCommand, TranscodeJobRegistry, audio_command};
use jellyfin_model::{
    EncodingContext, MediaStream, MediaStreamType, SubtitleDeliveryMethod, TranscodeReason,
};

use crate::{ApiError, AppState, authentication, encoding_runtime, stream_options::StreamOptions};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamQuery {
    #[serde(rename = "container", alias = "Container")]
    container: Option<String>,
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
    #[serde(rename = "params", alias = "Params")]
    params: Option<String>,
    #[serde(rename = "tag", alias = "Tag")]
    tag: Option<String>,
    #[serde(
        rename = "deviceProfileId",
        alias = "DeviceProfileId",
        alias = "deviceprofileid"
    )]
    _device_profile_id: Option<String>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(
        rename = "playSessionId",
        alias = "PlaySessionId",
        alias = "playsessionid"
    )]
    play_session_id: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId", alias = "deviceid")]
    device_id: Option<String>,
    #[serde(
        rename = "segmentContainer",
        alias = "SegmentContainer",
        alias = "segmentcontainer"
    )]
    segment_container: Option<String>,
    #[serde(
        rename = "segmentLength",
        alias = "SegmentLength",
        alias = "segmentlength"
    )]
    _segment_length: Option<i32>,
    #[serde(rename = "minSegments", alias = "MinSegments", alias = "minsegments")]
    _min_segments: Option<i32>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "enableAutoStreamCopy",
        alias = "EnableAutoStreamCopy",
        alias = "enableautostreamcopy"
    )]
    _enable_auto_stream_copy: Option<bool>,
    #[serde(
        rename = "allowVideoStreamCopy",
        alias = "AllowVideoStreamCopy",
        alias = "allowvideostreamcopy"
    )]
    _allow_video_stream_copy: Option<bool>,
    #[serde(
        rename = "allowAudioStreamCopy",
        alias = "AllowAudioStreamCopy",
        alias = "allowaudiostreamcopy"
    )]
    _allow_audio_stream_copy: Option<bool>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
        alias = "AudioBitrate",
        alias = "audioBitrate",
        alias = "audiobitrate"
    )]
    audio_bitrate: Option<i32>,
    #[serde(
        rename = "audioSampleRate",
        alias = "AudioSampleRate",
        alias = "audiosamplerate"
    )]
    audio_sample_rate: Option<i32>,
    #[serde(
        rename = "maxAudioBitDepth",
        alias = "MaxAudioBitDepth",
        alias = "maxaudiobitdepth"
    )]
    _max_audio_bit_depth: Option<i32>,
    #[serde(
        rename = "audioChannels",
        alias = "AudioChannels",
        alias = "audiochannels"
    )]
    audio_channels: Option<i32>,
    #[serde(
        rename = "maxAudioChannels",
        alias = "MaxAudioChannels",
        alias = "maxaudiochannels"
    )]
    max_audio_channels: Option<i32>,
    #[serde(rename = "profile", alias = "Profile")]
    _profile: Option<String>,
    #[serde(rename = "level", alias = "Level")]
    level: Option<String>,
    #[serde(rename = "framerate", alias = "Framerate")]
    _framerate: Option<f32>,
    #[serde(
        rename = "maxFramerate",
        alias = "MaxFramerate",
        alias = "maxframerate"
    )]
    _max_framerate: Option<f32>,
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "videoStreamIndex",
        alias = "VideoStreamIndex",
        alias = "videostreamindex"
    )]
    _video_stream_index: Option<i32>,
    #[serde(
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels",
        alias = "transcodingmaxaudiochannels"
    )]
    transcoding_max_audio_channels: Option<i32>,
    #[serde(rename = "width", alias = "Width")]
    _width: Option<i32>,
    #[serde(rename = "height", alias = "Height")]
    _height: Option<i32>,
    #[serde(
        rename = "videoBitRate",
        alias = "VideoBitRate",
        alias = "VideoBitrate",
        alias = "videoBitrate",
        alias = "videobitrate"
    )]
    _video_bitrate: Option<i32>,
    #[serde(
        rename = "subtitleStreamIndex",
        alias = "SubtitleStreamIndex",
        alias = "subtitlestreamindex"
    )]
    _subtitle_stream_index: Option<i32>,
    #[serde(
        default,
        rename = "subtitleMethod",
        alias = "SubtitleMethod",
        alias = "subtitlemethod",
        deserialize_with = "crate::query::optional_subtitle_delivery_method"
    )]
    _subtitle_method: Option<SubtitleDeliveryMethod>,
    #[serde(
        rename = "maxRefFrames",
        alias = "MaxRefFrames",
        alias = "maxrefframes"
    )]
    _max_ref_frames: Option<i32>,
    #[serde(
        rename = "maxVideoBitDepth",
        alias = "MaxVideoBitDepth",
        alias = "maxvideobitdepth"
    )]
    _max_video_bit_depth: Option<i32>,
    #[serde(rename = "requireAvc", alias = "RequireAvc", alias = "requireavc")]
    _require_avc: Option<bool>,
    #[serde(rename = "deInterlace", alias = "DeInterlace", alias = "deinterlace")]
    _de_interlace: Option<bool>,
    #[serde(
        rename = "requireNonAnamorphic",
        alias = "RequireNonAnamorphic",
        alias = "requirenonanamorphic"
    )]
    _require_non_anamorphic: Option<bool>,
    #[serde(
        rename = "startTimeTicks",
        alias = "StartTimeTicks",
        alias = "starttimeticks"
    )]
    start_time_ticks: Option<i64>,
    #[serde(
        rename = "copyTimestamps",
        alias = "CopyTimestamps",
        alias = "copytimestamps"
    )]
    copy_timestamps: Option<bool>,
    #[serde(
        rename = "cpuCoreLimit",
        alias = "CpuCoreLimit",
        alias = "cpucorelimit"
    )]
    cpu_core_limit: Option<i32>,
    #[serde(
        rename = "liveStreamId",
        alias = "LiveStreamId",
        alias = "livestreamid"
    )]
    live_stream_id: Option<String>,
    #[serde(
        rename = "enableMpegtsM2TsMode",
        alias = "EnableMpegtsM2TsMode",
        alias = "enablempegtsm2tsmode"
    )]
    _enable_mpegts_m2_ts_mode: Option<bool>,
    #[serde(rename = "videoCodec", alias = "VideoCodec", alias = "videocodec")]
    video_codec: Option<String>,
    #[serde(
        rename = "subtitleCodec",
        alias = "SubtitleCodec",
        alias = "subtitlecodec"
    )]
    subtitle_codec: Option<String>,
    #[serde(
        rename = "transcodeReasons",
        alias = "TranscodeReasons",
        alias = "transcodereasons"
    )]
    transcode_reasons: Option<String>,
    #[serde(
        default,
        rename = "context",
        alias = "Context",
        deserialize_with = "crate::query::optional_encoding_context"
    )]
    _context: Option<EncodingContext>,
    #[serde(
        default,
        rename = "streamOptions",
        alias = "StreamOptions",
        alias = "streamoptions"
    )]
    _stream_options: Vec<String>,
    #[serde(
        rename = "enableAudioVbrEncoding",
        alias = "EnableAudioVbrEncoding",
        alias = "enableaudiovbrencoding"
    )]
    enable_audio_vbr_encoding: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UniversalQuery {
    #[serde(
        default,
        rename = "container",
        alias = "Container",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    container: Vec<String>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId", alias = "deviceid")]
    device_id: Option<String>,
    #[serde(rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "maxAudioChannels",
        alias = "MaxAudioChannels",
        alias = "maxaudiochannels"
    )]
    max_audio_channels: Option<i32>,
    #[serde(
        rename = "transcodingAudioChannels",
        alias = "TranscodingAudioChannels",
        alias = "transcodingaudiochannels"
    )]
    transcoding_audio_channels: Option<i32>,
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "maxStreamingBitrate",
        alias = "MaxStreamingBitrate",
        alias = "maxstreamingbitrate"
    )]
    max_streaming_bitrate: Option<i64>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
        alias = "AudioBitrate",
        alias = "audioBitrate",
        alias = "audiobitrate"
    )]
    audio_bitrate: Option<i64>,
    #[serde(
        rename = "startTimeTicks",
        alias = "StartTimeTicks",
        alias = "starttimeticks"
    )]
    start_time_ticks: Option<i64>,
    #[serde(
        rename = "transcodingContainer",
        alias = "TranscodingContainer",
        alias = "transcodingcontainer"
    )]
    transcoding_container: Option<String>,
    #[serde(
        rename = "transcodingProtocol",
        alias = "TranscodingProtocol",
        alias = "transcodingprotocol"
    )]
    transcoding_protocol: Option<jellyfin_model::MediaStreamProtocol>,
    #[serde(
        rename = "maxAudioSampleRate",
        alias = "MaxAudioSampleRate",
        alias = "maxaudiosamplerate"
    )]
    max_audio_sample_rate: Option<i32>,
    #[serde(
        rename = "maxAudioBitDepth",
        alias = "MaxAudioBitDepth",
        alias = "maxaudiobitdepth"
    )]
    max_audio_bit_depth: Option<i32>,
    #[serde(
        rename = "enableRemoteMedia",
        alias = "EnableRemoteMedia",
        alias = "enableremotemedia"
    )]
    enable_remote_media: Option<bool>,
    #[serde(
        rename = "enableAudioVbrEncoding",
        alias = "EnableAudioVbrEncoding",
        alias = "enableaudiovbrencoding"
    )]
    enable_audio_vbr_encoding: Option<bool>,
    #[serde(
        rename = "enableRedirection",
        alias = "EnableRedirection",
        alias = "enableredirection"
    )]
    enable_redirection: Option<bool>,
}

pub(crate) async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<StreamQuery>, QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, None, query, request).await
}

pub(crate) async fn stream_with_container(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, container)): Path<(Uuid, String)>,
    query: Result<Query<StreamQuery>, QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, Some(&container), query, request).await
}

pub(crate) async fn universal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UniversalQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity =
        authentication::authenticated_identity(&state, &headers, Some(request.uri())).await?;
    let requested_user_id = query.user_id.filter(|user_id| !user_id.is_nil());
    let (target_user_id, target_policy, item) = match &identity {
        authentication::AuthenticatedIdentity::Device(session) => {
            let target_user_id = requested_user_id.unwrap_or(session.user.id);
            if target_user_id != session.user.id && !session.user.is_administrator {
                return Err(ApiError::Forbidden);
            }
            let policy: jellyfin_model::UserPolicy = if target_user_id == session.user.id {
                serde_json::from_value(session.user.policy.clone())
                    .map_err(|_| ApiError::Internal)?
            } else {
                serde_json::from_value(state.users.get(target_user_id).await?.policy)
                    .map_err(|_| ApiError::Internal)?
            };
            let item = state
                .user_library
                .item(&session.user, target_user_id, item_id)
                .await?;
            (Some(target_user_id), Some(policy), item)
        }
        // An API key has the default unrestricted controller authorization.
        // If it explicitly selects a user, use that user's normal library and
        // transcoding policy just like other playback endpoints do.
        authentication::AuthenticatedIdentity::ApiKey(_) => match requested_user_id {
            Some(target_user_id) => {
                let user = state.users.get(target_user_id).await?;
                let policy =
                    serde_json::from_value(user.policy.clone()).map_err(|_| ApiError::Internal)?;
                let item = state
                    .user_library
                    .item(&user, target_user_id, item_id)
                    .await?;
                (Some(target_user_id), Some(policy), item)
            }
            None => (
                None,
                None,
                state
                    .base_items
                    .get(item_id)
                    .await?
                    .ok_or(ApiError::NotFound)?,
            ),
        },
    };
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let item = if let Some(media_source_id) = query
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let source_id = Uuid::parse_str(media_source_id).map_err(|_| ApiError::NotFound)?;
        if source_id == item_id {
            item
        } else {
            state
                .base_items
                .alternate_media_version(item_id, source_id)
                .await?
                .ok_or(ApiError::NotFound)?
        }
    } else {
        item
    };
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let path = jellyfin_controller::media_source_path(&item).ok_or(ApiError::NotFound)?;
    let actual_container = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    let streams = state
        .media_streams
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item.id))
        .await?;
    let supports_direct = supports_direct_play(&query, actual_container, &streams);
    let requires_transcode = universal_requires_transcode(&query);
    if supports_direct && !requires_transcode {
        if path.starts_with("http://") || path.starts_with("https://") {
            // The official controller only exposes an upstream URL when both
            // client capabilities opt in.  Otherwise Jellyfin remains the
            // streaming endpoint and proxies the authorized remote source.
            if should_redirect_remote_media(&query) {
                return Ok(Redirect::temporary(path).into_response());
            }
            return crate::videos::proxy_remote_stream(
                &state.remote_stream_client,
                &headers,
                item.id,
                path,
                crate::videos::required_remote_user_agent(&item),
            )
            .await;
        }
        return serve_path(headers, path, request).await;
    }

    if target_policy.is_some_and(|policy| !policy.enable_audio_playback_transcoding) {
        return Err(ApiError::Forbidden);
    }

    let play_session_id = Uuid::new_v4().simple().to_string();
    if universal_uses_hls(&query) {
        let hls_query = crate::hls_segment::TranscodeQuery::universal_audio(
            query.media_source_id.clone(),
            query.device_id.clone(),
            Some(play_session_id.clone()),
            target_user_id,
            query.audio_codec.clone(),
            query.audio_bitrate.or(query.max_streaming_bitrate),
            query.max_audio_channels,
            query.max_audio_sample_rate,
            query.audio_stream_index,
            query.start_time_ticks,
            query.enable_audio_vbr_encoding,
        );
        let hls_uri = hls_query.audio_master_uri(item_id, identity.access_token())?;
        return crate::hls_segment::ensure_master_playlist(
            &state, headers, &hls_uri, hls_query, &identity,
        )
        .await;
    }

    let codec = query.audio_codec.as_deref().unwrap_or("aac").to_owned();
    let container = query.transcoding_container.as_deref().map_or_else(
        || audio_container(&codec).to_owned(),
        |container| container.trim_start_matches('.').to_owned(),
    );
    let output = state.transcode_directory.join(format!(
        "{item_id}-audio-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let source_audio = streams.iter().find(|stream| {
        stream.stream_type == MediaStreamType::Audio
            && query
                .audio_stream_index
                .is_none_or(|index| stream.index == index)
    });
    let channels = source_audio.and_then(|source| {
        encoding_runtime::output_audio_channels(
            &codec,
            source.channels,
            None,
            query.max_audio_channels,
            None,
            query.transcoding_audio_channels,
        )
    });
    let requested_bitrate = query.audio_bitrate.or(query.max_streaming_bitrate);
    let bitrate = encoding_runtime::output_audio_bitrate(
        &codec,
        source_audio.is_some(),
        source_audio.and_then(|source| source.channels),
        channels,
        requested_bitrate,
    );
    let encoding_options = crate::configuration::encoding_runtime_options(&state).await?;
    let mut command = audio_command(
        &state.ffmpeg_path,
        std::path::Path::new(path),
        &output,
        &codec,
        bitrate,
        channels,
        query.max_audio_sample_rate,
        query.audio_stream_index,
        query.start_time_ticks,
        true,
    );
    encoding_runtime::apply_audio_vbr(
        &mut command,
        &codec,
        bitrate,
        channels,
        encoding_runtime::audio_vbr_enabled(
            encoding_options.enable_audio_vbr,
            query.enable_audio_vbr_encoding,
        ),
    );
    encoding_runtime::apply_thread_count(
        &mut command,
        None,
        encoding_options.encoding_thread_count,
    );
    serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
        Arc::clone(&state.transcode_jobs),
        query.device_id,
        Some(play_session_id),
        false,
        codec.eq_ignore_ascii_case("copy"),
        TranscodeReason::NONE,
    )
}

fn universal_requires_transcode(query: &UniversalQuery) -> bool {
    query.audio_codec.is_some()
        || query.max_audio_channels.is_some()
        || query.transcoding_audio_channels.is_some()
        || query.audio_stream_index.is_some()
        || query.max_streaming_bitrate.is_some()
        || query.audio_bitrate.is_some()
        || query.max_audio_sample_rate.is_some()
        || query.start_time_ticks.is_some_and(|ticks| ticks != 0)
        || query.transcoding_container.is_some()
}

fn should_redirect_remote_media(query: &UniversalQuery) -> bool {
    query.enable_remote_media == Some(true) && query.enable_redirection == Some(true)
}

fn universal_uses_hls(query: &UniversalQuery) -> bool {
    query.transcoding_protocol == Some(jellyfin_model::MediaStreamProtocol::Hls)
}

/// Mirrors UniversalAudioController's DirectPlayProfile construction: each
/// comma-delimited `container` value is a `container|codec|codec` profile.
/// A profile without codecs permits the matching container. When scan metadata
/// identifies the selected audio codec, a codec-constrained profile must name
/// it before raw bytes can be sent to the player.
fn supports_direct_play(
    query: &UniversalQuery,
    actual_container: &str,
    streams: &[MediaStream],
) -> bool {
    let selected_stream = streams.iter().find(|stream| {
        stream.stream_type == MediaStreamType::Audio
            && query
                .audio_stream_index
                .is_none_or(|index| stream.index == index)
    });
    if selected_stream.is_some_and(|stream| {
        query
            .max_audio_bit_depth
            .is_some_and(|maximum| stream.bit_depth.is_some_and(|depth| depth > maximum))
    }) {
        return false;
    }
    let selected_codec = selected_stream
        .and_then(|stream| stream.codec.as_deref())
        .map(str::trim)
        .filter(|codec| !codec.is_empty());

    query.container.iter().any(|profile| {
        let mut parts = profile
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty());
        let Some(container) = parts.next() else {
            return false;
        };
        if !container.eq_ignore_ascii_case(actual_container) {
            return false;
        }
        let codecs = parts.collect::<Vec<_>>();
        codecs.is_empty()
            || selected_codec.is_none()
            || selected_codec.is_some_and(|codec| {
                codecs
                    .iter()
                    .any(|supported| supported.eq_ignore_ascii_case(codec))
            })
    })
}

fn audio_container(codec: &str) -> &str {
    match codec.to_ascii_lowercase().as_str() {
        "mp3" => "mp3",
        "flac" => "flac",
        "opus" => "opus",
        "aac" | "alac" => "m4a",
        "ac3" | "eac3" => "ac3",
        "wav" | "pcm_s16le" => "wav",
        _ => codec,
    }
}

fn codec_for_container(container: &str) -> &str {
    match container.to_ascii_lowercase().as_str() {
        "mp3" => "mp3",
        "flac" => "flac",
        "opus" | "ogg" => "opus",
        "ac3" => "ac3",
        "eac3" => "eac3",
        "wav" | "wave" => "pcm_s16le",
        _ => "aac",
    }
}

fn requested_stream_container<'a>(
    route_container: Option<&'a str>,
    query: &'a StreamQuery,
) -> Result<Option<&'a str>, ApiError> {
    let container = route_container.or(query.container.as_deref());
    if container.is_some_and(|container| {
        container.len() > 40
            || !container.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b',' | b'|')
            })
    }) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(container
        .map(|container| container.trim_start_matches('.'))
        .filter(|container| !container.is_empty()))
}

pub(crate) fn valid_encoding_name(value: &str) -> bool {
    value.len() <= 40
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b',' | b'|')
        })
}

pub(crate) fn valid_encoding_level(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    let mut parts = value.split('.');
    let Some(major) = parts.next() else {
        return false;
    };
    !major.is_empty()
        && major.bytes().all(|byte| byte.is_ascii_digit())
        && parts.next().is_none_or(|minor| {
            !minor.is_empty() && minor.bytes().all(|byte| byte.is_ascii_digit())
        })
        && parts.next().is_none()
}

fn validate_progressive_query(
    route_container: Option<&str>,
    query: &StreamQuery,
) -> Result<(), ApiError> {
    requested_stream_container(route_container, query)?;
    for value in [
        query.segment_container.as_deref(),
        query.audio_codec.as_deref(),
        query.video_codec.as_deref(),
        query.subtitle_codec.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !valid_encoding_name(value) {
            return Err(ApiError::InvalidRequest);
        }
    }
    if query
        .level
        .as_deref()
        .is_some_and(|value| !valid_encoding_level(value))
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(())
}

fn apply_legacy_params(query: &mut StreamQuery) -> Result<(), ApiError> {
    let Some(params) = query.params.clone().filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    for (index, value) in params.split(';').enumerate() {
        if value.trim().is_empty() {
            continue;
        }
        match index {
            1 => query.device_id = Some(value.to_owned()),
            2 => query.media_source_id = Some(value.to_owned()),
            3 => query.static_stream = Some(value.eq_ignore_ascii_case("true")),
            5 if valid_encoding_name(value) => query.audio_codec = Some(value.to_owned()),
            9 => {
                query.audio_bitrate = Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            10 => {
                query.max_audio_channels =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            14 => {
                query.start_time_ticks =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            20 => query.play_session_id = Some(value.to_owned()),
            22 => query.live_stream_id = Some(value.to_owned()),
            26 => {
                query.transcoding_max_audio_channels =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            28 => query.tag = Some(value.to_owned()),
            30 if valid_encoding_name(value) => query.subtitle_codec = Some(value.to_owned()),
            33 => query.transcode_reasons = Some(value.to_owned()),
            _ => {}
        }
    }
    Ok(())
}

fn progressive_audio_target(
    route_container: Option<&str>,
    query: &StreamQuery,
) -> Result<(String, String), ApiError> {
    let requested_container = requested_stream_container(route_container, query)?;
    let codec = query
        .audio_codec
        .as_deref()
        .unwrap_or_else(|| codec_for_container(requested_container.unwrap_or("m4a")));
    let container = requested_container
        .map(str::to_owned)
        .unwrap_or_else(|| audio_container(codec).to_owned());
    Ok((codec.to_owned(), container))
}

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: Result<Query<StreamQuery>, QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity =
        authentication::authenticated_identity(&state, &headers, Some(request.uri())).await?;
    let stream_options = StreamOptions::from_uri(request.uri());
    let Query(mut query) = query.map_err(|_| ApiError::InvalidRequest)?;
    validate_progressive_query(requested_container, &query)?;
    apply_legacy_params(&mut query)?;
    let mut requested_item = match identity {
        authentication::AuthenticatedIdentity::Device(authenticated) => {
            state
                .user_library
                .item(&authenticated.user, authenticated.user.id, item_id)
                .await?
        }
        // API keys use the same unrestricted default authorization policy as
        // the official stream controller. They have no device user whose
        // library policy can be applied before opening the selected source.
        authentication::AuthenticatedIdentity::ApiKey(_) => state
            .base_items
            .get(item_id)
            .await?
            .ok_or(ApiError::NotFound)?,
    };
    let item_types = jellyfin_controller::ItemTypeRegistry::default();
    let item_type = item_types
        .resolve(&requested_item.item_type)
        .ok_or(ApiError::NotFound)?;
    requested_item.item_type = item_type.name().to_owned();
    if requested_item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let opened_source = query
        .live_stream_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|live_stream_id| {
            state
                .live_streams
                .get(item_id, live_stream_id)
                .ok_or(ApiError::NotFound)
        })
        .transpose()?;
    let item = if opened_source.is_some() {
        requested_item
    } else if let Some(media_source_id) = query
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        crate::media_source::resolve_static_item(&state, requested_item, media_source_id).await?
    } else {
        requested_item
    };
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let path = opened_source
        .as_ref()
        .and_then(|source| source.path.clone())
        .or_else(|| jellyfin_controller::media_source_path(&item).map(str::to_owned))
        .ok_or(ApiError::NotFound)?;
    let requested_container = requested_stream_container(requested_container, &query)?;
    if query.static_stream.unwrap_or(false)
        && let Some(container) = requested_container
    {
        let actual = std::path::Path::new(&path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        if !container.eq_ignore_ascii_case(actual) {
            return Err(ApiError::UnsupportedMediaType);
        }
    }
    if query.static_stream.unwrap_or(false) {
        if path.starts_with("http://") || path.starts_with("https://") {
            return crate::videos::proxy_remote_stream(
                &state.remote_stream_client,
                &headers,
                item.id,
                &path,
                crate::videos::required_remote_user_agent(&item),
            )
            .await;
        }
        return serve_path(headers, &path, request).await;
    }

    let (codec, container) = progressive_audio_target(requested_container, &query)?;
    let output = state.transcode_directory.join(format!(
        "{item_id}-audio-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let streams = if let Some(source) = opened_source.as_ref() {
        source.media_streams.clone()
    } else {
        state
            .media_streams
            .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item.id))
            .await?
    };
    let source_audio = streams.iter().find(|stream| {
        stream.stream_type == MediaStreamType::Audio
            && query
                .audio_stream_index
                .is_none_or(|index| stream.index == index)
    });
    let option_channels = stream_options
        .get_request_option(&codec, "audiochannels")
        .and_then(|value| value.parse().ok());
    let channels = source_audio.and_then(|source| {
        encoding_runtime::output_audio_channels(
            &codec,
            source.channels,
            option_channels,
            query.max_audio_channels,
            query.audio_channels,
            query.transcoding_max_audio_channels,
        )
    });
    let bitrate = encoding_runtime::output_audio_bitrate(
        &codec,
        source_audio.is_some(),
        source_audio.and_then(|source| source.channels),
        channels,
        query.audio_bitrate.map(i64::from),
    );
    let encoding_options = crate::configuration::encoding_runtime_options(&state).await?;
    let mut command = audio_command(
        &state.ffmpeg_path,
        std::path::Path::new(&path),
        &output,
        &codec,
        bitrate,
        channels,
        query.audio_sample_rate,
        query.audio_stream_index,
        query.start_time_ticks,
        query.copy_timestamps.unwrap_or(false),
    );
    encoding_runtime::apply_audio_vbr(
        &mut command,
        &codec,
        bitrate,
        channels,
        encoding_runtime::audio_vbr_enabled(
            encoding_options.enable_audio_vbr,
            query.enable_audio_vbr_encoding,
        ),
    );
    encoding_runtime::apply_thread_count(
        &mut command,
        query.cpu_core_limit,
        encoding_options.encoding_thread_count,
    );
    serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
        Arc::clone(&state.transcode_jobs),
        query.device_id,
        query.play_session_id,
        false,
        codec.eq_ignore_ascii_case("copy"),
        query
            .transcode_reasons
            .as_deref()
            .and_then(TranscodeReason::parse_names)
            .unwrap_or(TranscodeReason::NONE),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use axum::{body::to_bytes, http::Uri};
    use axum_extra::extract::Query;
    use jellyfin_controller::{FfmpegCommand, TranscodeJobRegistry};
    use jellyfin_model::{EncodingContext, SubtitleDeliveryMethod, TranscodeReason};

    use super::{
        StreamQuery, UniversalQuery, apply_legacy_params, progressive_audio_target,
        requested_stream_container, serve_transcoded_path, should_redirect_remote_media,
        supports_direct_play, universal_requires_transcode, universal_uses_hls,
        validate_progressive_query,
    };

    #[test]
    fn audio_stream_binds_alternate_media_source_id_case_insensitively() {
        for key in ["MediaSourceId", "mediaSourceId", "mediasourceid"] {
            let uri: Uri = format!("/Audio/item/stream?{key}=alternate")
                .parse()
                .unwrap();
            let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(query.media_source_id.as_deref(), Some("alternate"));
        }
    }

    #[test]
    fn audio_stream_binds_android_transcoding_parameters() {
        let uri: Uri = "/audio/item/stream?Container=mp3&static=false&params=legacy&Tag=etag&deviceprofileid=profile&segmentContainer=ts&SegmentLength=6&minsegments=2&audioCodec=mp3&enableAutoStreamCopy=true&AllowVideoStreamCopy=false&allowaudiostreamcopy=true&AudioBitrate=192000&audioSampleRate=44100&MaxAudioBitDepth=24&audioChannels=2&maxAudioChannels=2&Profile=main&Level=4.1&framerate=24&MaxFramerate=30&width=1280&Height=720&videoBitrate=2000000&subtitleStreamIndex=3&subtitleMethod=1&MaxRefFrames=4&maxvideobitdepth=10&RequireAvc=true&deinterlace=true&requireNonAnamorphic=true&audioStreamIndex=1&videoStreamIndex=0&startTimeTicks=10000&CopyTimestamps=true&PlaySessionId=play-session&deviceid=device-1&transcodingMaxAudioChannels=6&cpuCoreLimit=2&liveStreamId=live&enableMpegtsM2TsMode=true&videoCodec=h264&subtitleCodec=srt&transcodeReasons=ContainerNotSupported&context=static&streamOptions=quality%3Dhigh&enableAudioVbrEncoding=false"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.container.as_deref(), Some("mp3"));
        assert!(!query.static_stream.unwrap());
        assert_eq!(query.params.as_deref(), Some("legacy"));
        assert_eq!(query.tag.as_deref(), Some("etag"));
        assert_eq!(query._device_profile_id.as_deref(), Some("profile"));
        assert_eq!(query.segment_container.as_deref(), Some("ts"));
        assert_eq!(query._segment_length, Some(6));
        assert_eq!(query._min_segments, Some(2));
        assert_eq!(query.audio_codec.as_deref(), Some("mp3"));
        assert_eq!(query._enable_auto_stream_copy, Some(true));
        assert_eq!(query._allow_video_stream_copy, Some(false));
        assert_eq!(query._allow_audio_stream_copy, Some(true));
        assert_eq!(query.audio_bitrate, Some(192000));
        assert_eq!(query.audio_sample_rate, Some(44100));
        assert_eq!(query._max_audio_bit_depth, Some(24));
        assert_eq!(query.audio_channels, Some(2));
        assert_eq!(query.max_audio_channels, Some(2));
        assert_eq!(query._profile.as_deref(), Some("main"));
        assert_eq!(query.level.as_deref(), Some("4.1"));
        assert_eq!(query._framerate, Some(24.0));
        assert_eq!(query._max_framerate, Some(30.0));
        assert_eq!(query._width, Some(1280));
        assert_eq!(query._height, Some(720));
        assert_eq!(query._video_bitrate, Some(2_000_000));
        assert_eq!(query._subtitle_stream_index, Some(3));
        assert_eq!(query._subtitle_method, Some(SubtitleDeliveryMethod::Embed));
        assert_eq!(query._max_ref_frames, Some(4));
        assert_eq!(query._max_video_bit_depth, Some(10));
        assert_eq!(query._require_avc, Some(true));
        assert_eq!(query._de_interlace, Some(true));
        assert_eq!(query._require_non_anamorphic, Some(true));
        assert_eq!(query.audio_stream_index, Some(1));
        assert_eq!(query._video_stream_index, Some(0));
        assert_eq!(query.start_time_ticks, Some(10000));
        assert_eq!(query.copy_timestamps, Some(true));
        assert_eq!(query.play_session_id.as_deref(), Some("play-session"));
        assert_eq!(query.device_id.as_deref(), Some("device-1"));
        assert_eq!(query.transcoding_max_audio_channels, Some(6));
        assert_eq!(query.cpu_core_limit, Some(2));
        assert_eq!(query.live_stream_id.as_deref(), Some("live"));
        assert_eq!(query._enable_mpegts_m2_ts_mode, Some(true));
        assert_eq!(query.video_codec.as_deref(), Some("h264"));
        assert_eq!(query.subtitle_codec.as_deref(), Some("srt"));
        assert_eq!(
            query.transcode_reasons.as_deref(),
            Some("ContainerNotSupported")
        );
        assert_eq!(query._context, Some(EncodingContext::Static));
        assert_eq!(query._stream_options, ["quality=high"]);
        assert_eq!(query.enable_audio_vbr_encoding, Some(false));
    }

    #[test]
    fn progressive_audio_rejects_unknown_enums_and_out_of_range_int32() {
        for query_string in [
            "subtitleMethod=Unknown",
            "subtitleMethod=5",
            "context=Download",
            "context=2",
            "maxAudioBitDepth=2147483648",
            "audioBitRate=2147483648",
            "videoBitRate=2147483648",
        ] {
            let uri: Uri = format!("/audio/item/stream?{query_string}")
                .parse()
                .unwrap();
            assert!(
                Query::<StreamQuery>::try_from_uri(&uri).is_err(),
                "{query_string}"
            );
        }
    }

    #[test]
    fn progressive_audio_applies_the_official_cpu_core_limit() {
        let mut command = jellyfin_controller::audio_command(
            std::path::Path::new("/usr/bin/ffmpeg"),
            std::path::Path::new("/media/song.flac"),
            std::path::Path::new("/tmp/out.mp3"),
            "mp3",
            None,
            None,
            None,
            None,
            None,
            false,
        );
        crate::videos::apply_cpu_core_limit(&mut command, Some(1));
        assert!(
            command
                .arguments
                .windows(2)
                .any(|pair| pair == ["-threads", "1"])
        );
    }

    #[test]
    fn progressive_audio_applies_supported_legacy_params_over_query_values() {
        let mut values = vec![""; 34];
        values[1] = "legacy-device";
        values[2] = "legacy-source";
        values[3] = "true";
        values[5] = "mp3";
        values[9] = " 192000 ";
        values[10] = " 2 ";
        values[14] = " 10000 ";
        values[20] = "legacy-session";
        values[22] = "legacy-live";
        values[26] = " 6 ";
        values[28] = "legacy-tag";
        values[30] = "srt";
        values[33] = "ContainerNotSupported";
        let mut query = StreamQuery {
            device_id: Some("query-device".to_owned()),
            params: Some(values.join(";")),
            ..StreamQuery::default()
        };
        apply_legacy_params(&mut query).unwrap();
        assert_eq!(query.device_id.as_deref(), Some("legacy-device"));
        assert_eq!(query.media_source_id.as_deref(), Some("legacy-source"));
        assert_eq!(query.static_stream, Some(true));
        assert_eq!(query.audio_codec.as_deref(), Some("mp3"));
        assert_eq!(query.audio_bitrate, Some(192_000));
        assert_eq!(query.max_audio_channels, Some(2));
        assert_eq!(query.start_time_ticks, Some(10_000));
        assert_eq!(query.play_session_id.as_deref(), Some("legacy-session"));
        assert_eq!(query.live_stream_id.as_deref(), Some("legacy-live"));
        assert_eq!(query.transcoding_max_audio_channels, Some(6));
        assert_eq!(query.tag.as_deref(), Some("legacy-tag"));
        assert_eq!(query.subtitle_codec.as_deref(), Some("srt"));
        assert_eq!(
            query.transcode_reasons.as_deref(),
            Some("ContainerNotSupported")
        );
    }

    #[test]
    fn universal_audio_binds_sdk_parameters_case_insensitively() {
        let uri: Uri = "/audio/item/universal?mediasourceid=alternate&transcodingAudioChannels=2&AudioBitRate=128000&maxAudioSampleRate=48000&maxAudioBitDepth=24&transcodingProtocol=hls&enableRemoteMedia=true&enableRedirection=false"
            .parse()
            .unwrap();
        let query = Query::<super::UniversalQuery>::try_from_uri(&uri)
            .unwrap()
            .0;
        assert_eq!(query.media_source_id.as_deref(), Some("alternate"));
        assert_eq!(query.transcoding_audio_channels, Some(2));
        assert_eq!(query.audio_bitrate, Some(128000));
        assert_eq!(query.max_audio_sample_rate, Some(48000));
        assert_eq!(query.max_audio_bit_depth, Some(24));
        assert_eq!(
            query.transcoding_protocol,
            Some(jellyfin_model::MediaStreamProtocol::Hls)
        );
        assert_eq!(query.enable_remote_media, Some(true));
        assert_eq!(query.enable_redirection, Some(false));
    }

    #[test]
    fn universal_audio_honors_camel_case_audio_bitrate() {
        // ASP.NET binds `audioBitRate` case-insensitively, so clients using the conventional
        // camel-case spelling must not fall through to direct playback.
        let uri: Uri = "/audio/item/universal?audioBitrate=128000".parse().unwrap();
        let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.audio_bitrate, Some(128_000));
        assert!(universal_requires_transcode(&query));
    }

    #[test]
    fn universal_audio_redirects_remote_media_only_when_both_capabilities_opt_in() {
        for (query_string, expected) in [
            ("", false),
            ("enableRemoteMedia=true", false),
            ("enableRedirection=true", false),
            ("enableRemoteMedia=true&enableRedirection=true", true),
        ] {
            let uri: Uri = format!("/audio/item/universal?{query_string}")
                .parse()
                .unwrap();
            let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(
                should_redirect_remote_media(&query),
                expected,
                "{query_string}"
            );
        }
    }

    #[test]
    fn universal_audio_selects_hls_only_when_requested() {
        for (query_string, expected) in [
            ("", false),
            ("transcodingProtocol=http", false),
            ("transcodingProtocol=hls", true),
        ] {
            let uri: Uri = format!("/audio/item/universal?{query_string}")
                .parse()
                .unwrap();
            let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(universal_uses_hls(&query), expected, "{query_string}");
        }
    }

    #[test]
    fn universal_audio_direct_play_honors_profile_audio_codecs() {
        let stream = jellyfin_model::MediaStream {
            stream_type: jellyfin_model::MediaStreamType::Audio,
            index: 0,
            codec: Some("flac".to_owned()),
            ..Default::default()
        };
        for (profile, expected) in [
            ("flac", true),
            ("flac|flac", true),
            ("flac|aac", false),
            ("mp3|flac", false),
        ] {
            let uri: Uri = format!("/audio/item/universal?container={profile}")
                .parse()
                .unwrap();
            let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(
                supports_direct_play(&query, "flac", &[stream.clone()]),
                expected,
                "{profile}"
            );
        }
    }

    #[test]
    fn progressive_audio_honors_camel_case_audio_bitrate() {
        let uri: Uri = "/audio/item/stream?audioBitrate=192000".parse().unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.audio_bitrate, Some(192_000));
    }

    #[test]
    fn progressive_audio_binds_query_container_case_insensitively() {
        for key in ["container", "Container"] {
            let uri: Uri = format!("/audio/item/stream?{key}=flac").parse().unwrap();
            let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(query.container.as_deref(), Some("flac"));
        }
    }

    #[test]
    fn progressive_audio_selects_the_requested_container_and_compatible_codec() {
        let query = StreamQuery {
            container: Some("flac".to_owned()),
            ..StreamQuery::default()
        };
        assert_eq!(
            progressive_audio_target(None, &query).unwrap(),
            ("flac".to_owned(), "flac".to_owned())
        );

        let explicit_codec = StreamQuery {
            container: Some("ogg".to_owned()),
            audio_codec: Some("vorbis".to_owned()),
            ..StreamQuery::default()
        };
        assert_eq!(
            progressive_audio_target(None, &explicit_codec).unwrap(),
            ("vorbis".to_owned(), "ogg".to_owned())
        );

        assert_eq!(
            progressive_audio_target(Some("mp3"), &query).unwrap(),
            ("mp3".to_owned(), "mp3".to_owned())
        );
    }

    #[test]
    fn progressive_audio_rejects_invalid_output_containers() {
        let query = StreamQuery {
            container: Some("../../outside".to_owned()),
            ..StreamQuery::default()
        };
        assert!(requested_stream_container(None, &query).is_err());

        let query = StreamQuery {
            container: Some("a".repeat(41)),
            ..StreamQuery::default()
        };
        assert!(requested_stream_container(None, &query).is_err());
    }

    #[test]
    fn progressive_audio_validates_sdk_codec_and_level_parameters() {
        for query in [
            StreamQuery {
                audio_codec: Some("aac;touch".to_owned()),
                ..StreamQuery::default()
            },
            StreamQuery {
                subtitle_codec: Some("srt/ass".to_owned()),
                ..StreamQuery::default()
            },
            StreamQuery {
                level: Some("4.1.2".to_owned()),
                ..StreamQuery::default()
            },
        ] {
            assert!(validate_progressive_query(None, &query).is_err());
        }
        assert!(validate_progressive_query(None, &StreamQuery::default()).is_ok());
    }

    #[tokio::test]
    async fn progressive_transcode_registers_streams_and_cleans_up() {
        let output = std::env::temp_dir().join(format!(
            "jellyfin-progressive-{}.mp3",
            uuid::Uuid::new_v4().simple()
        ));
        let command = FfmpegCommand {
            program: PathBuf::from("sh"),
            arguments: vec![
                "-c".to_owned(),
                "printf progressive > \"$1\"; sleep 0.1".to_owned(),
                "jellyfin-progressive-test".to_owned(),
                output.to_string_lossy().into_owned(),
            ],
        };
        let registry = std::sync::Arc::new(TranscodeJobRegistry::new());

        let response = serve_transcoded_path(
            command,
            &output.to_string_lossy(),
            false,
            std::sync::Arc::clone(&registry),
            Some("device-1".to_owned()),
            Some("play-session-1".to_owned()),
            false,
            true,
            TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED
                | TranscodeReason::AUDIO_BITRATE_NOT_SUPPORTED,
        )
        .unwrap();
        let info = registry.get("PLAY-SESSION-1").expect("registered job");
        assert_eq!(info.device_id.as_deref(), Some("device-1"));
        assert_eq!(
            info.path.as_deref(),
            Some(output.to_string_lossy().as_ref())
        );
        assert!(!info.is_hls);
        assert!(!info.is_video_direct);
        assert!(info.is_audio_direct);
        assert_eq!(
            info.transcode_reasons.bits(),
            (TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED
                | TranscodeReason::AUDIO_BITRATE_NOT_SUPPORTED)
                .bits()
        );

        let body = to_bytes(response.into_body(), 1024).await.unwrap();

        assert_eq!(&body[..], b"progressive");
        assert!(registry.list().is_empty());
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn dropping_progressive_response_cancels_and_cleans_up() {
        let output = std::env::temp_dir().join(format!(
            "jellyfin-progressive-drop-{}.mp3",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::write(&output, b"partial").await.unwrap();
        let command = FfmpegCommand {
            program: PathBuf::from("sleep"),
            arguments: vec!["30".to_owned()],
        };
        let registry = std::sync::Arc::new(TranscodeJobRegistry::new());
        let response = serve_transcoded_path(
            command,
            &output.to_string_lossy(),
            false,
            std::sync::Arc::clone(&registry),
            Some("device-1".to_owned()),
            Some("drop-session".to_owned()),
            false,
            false,
            TranscodeReason::NONE,
        )
        .unwrap();
        assert!(registry.get("drop-session").is_some());

        drop(response);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(registry.get("drop-session").is_none());
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn stopping_progressive_session_cancels_its_process() {
        let output = std::env::temp_dir().join(format!(
            "jellyfin-progressive-stop-{}.mp3",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::write(&output, b"partial").await.unwrap();
        let command = FfmpegCommand {
            program: PathBuf::from("sleep"),
            arguments: vec!["30".to_owned()],
        };
        let registry = std::sync::Arc::new(TranscodeJobRegistry::new());
        let response = serve_transcoded_path(
            command,
            &output.to_string_lossy(),
            false,
            std::sync::Arc::clone(&registry),
            Some("device-1".to_owned()),
            Some("stop-session".to_owned()),
            false,
            false,
            TranscodeReason::NONE,
        )
        .unwrap();

        let stopped = registry
            .stop_for_session("unrelated-device", "STOP-SESSION")
            .await;

        assert_eq!(stopped.len(), 1);
        assert!(registry.get("stop-session").is_none());
        drop(response);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn progressive_head_does_not_start_or_register_ffmpeg() {
        let output = std::env::temp_dir().join(format!(
            "jellyfin-progressive-head-{}.mp3",
            uuid::Uuid::new_v4().simple()
        ));
        let command = FfmpegCommand {
            program: PathBuf::from("definitely-not-an-ffmpeg-binary"),
            arguments: Vec::new(),
        };
        let registry = std::sync::Arc::new(TranscodeJobRegistry::new());

        let response = serve_transcoded_path(
            command,
            &output.to_string_lossy(),
            true,
            std::sync::Arc::clone(&registry),
            Some("device-1".to_owned()),
            Some("head-session".to_owned()),
            false,
            false,
            TranscodeReason::NONE,
        )
        .unwrap();

        assert_eq!(to_bytes(response.into_body(), 1).await.unwrap().len(), 0);
        assert!(registry.list().is_empty());
        assert!(!output.exists());
    }
}

pub(crate) async fn serve_path(
    mut headers: HeaderMap,
    path: &str,
    mut request: Request<Body>,
) -> Result<Response, ApiError> {
    request.headers_mut().clear();
    for name in [
        axum::http::header::RANGE,
        axum::http::header::IF_RANGE,
        axum::http::header::IF_MODIFIED_SINCE,
        axum::http::header::IF_UNMODIFIED_SINCE,
    ] {
        if let Some(value) = headers.remove(&name) {
            request.headers_mut().insert(name, value);
        }
    }
    let response = match ServeFile::new(path)
        .with_buf_chunk_size(64 * 1024)
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {},
    };
    Ok(response.map(Body::new))
}

/// Starts a progressive `FFmpeg` job and streams the output as it grows.
///
/// Jellyfin's progressive endpoints return the response before `FFmpeg` has
/// completed. Android's `ExoPlayer` relies on that behavior for long files.
pub(crate) fn serve_transcoded_path(
    command: FfmpegCommand,
    output_path: &str,
    is_head: bool,
    transcode_jobs: Arc<TranscodeJobRegistry>,
    device_id: Option<String>,
    play_session_id: Option<String>,
    is_video_direct: bool,
    is_audio_direct: bool,
    transcode_reasons: TranscodeReason,
) -> Result<Response, ApiError> {
    let content_type = jellyfin_model::MimeTypes::get_mime_type(output_path)
        .unwrap_or_else(|_| "application/octet-stream".to_owned());
    let response = Response::builder()
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(axum::http::header::ACCEPT_RANGES, "none");
    if is_head {
        return response.body(Body::empty()).map_err(|_| ApiError::Internal);
    }

    let child = Command::new(&command.program)
        .args(&command.arguments)
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ApiError::Internal)?;
    let job_id = Uuid::new_v4().simple().to_string();
    let job = transcode_jobs.register_progressive_with_path(
        &job_id,
        device_id.as_deref(),
        play_session_id.as_deref(),
        output_path,
    );
    transcode_jobs.set_direct_stream_flags(&job_id, is_video_direct, is_audio_direct);
    transcode_jobs.set_transcode_reasons(&job_id, transcode_reasons);
    job.mark_running();
    let process_jobs = Arc::clone(&transcode_jobs);
    let process_job_id = job_id.clone();
    let program = command.program.display().to_string();
    let process = tokio::spawn(async move {
        let result = drive_progressive_ffmpeg(child, &program, &job).await;
        process_jobs.remove(&process_job_id);
        result
    });
    let state = ProgressiveTranscodeState {
        process: Some(process),
        output_path: PathBuf::from(output_path),
        file: None,
        transcode_jobs,
        job_id,
    };
    let body = Body::from_stream(stream::unfold(state, next_transcode_chunk));
    response.body(body).map_err(|_| ApiError::Internal)
}

struct ProgressiveTranscodeState {
    process: Option<tokio::task::JoinHandle<Result<(), String>>>,
    output_path: PathBuf,
    file: Option<tokio::fs::File>,
    transcode_jobs: Arc<TranscodeJobRegistry>,
    job_id: String,
}

impl ProgressiveTranscodeState {
    async fn remove_output(&mut self) {
        self.file = None;
        let _ = tokio::fs::remove_file(&self.output_path).await;
    }
}

impl Drop for ProgressiveTranscodeState {
    fn drop(&mut self) {
        self.transcode_jobs.remove(&self.job_id);
        if let Some(process) = self.process.take() {
            process.abort();
            let output_path = self.output_path.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = process.await;
                    if !output_path.as_os_str().is_empty() {
                        let _ = tokio::fs::remove_file(output_path).await;
                    }
                });
            }
        }
    }
}

struct ProgressiveRunningGuard(TranscodeJobHandle);

impl Drop for ProgressiveRunningGuard {
    fn drop(&mut self) {
        self.0.mark_finished();
    }
}

async fn drive_progressive_ffmpeg(
    mut child: tokio::process::Child,
    program: &str,
    job: &TranscodeJobHandle,
) -> Result<(), String> {
    let _running = ProgressiveRunningGuard(job.clone());
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if job.cancellation_requested() {
            let _ = child.start_kill();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    if status.success() || job.cancellation_requested() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

async fn next_transcode_chunk(
    mut state: ProgressiveTranscodeState,
) -> Option<(Result<Bytes, io::Error>, ProgressiveTranscodeState)> {
    loop {
        if state.file.is_none() {
            match tokio::fs::File::open(&state.output_path).await {
                Ok(file) => state.file = Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if state
                        .process
                        .as_ref()
                        .is_some_and(tokio::task::JoinHandle::is_finished)
                    {
                        let result = state
                            .process
                            .as_mut()
                            .expect("progressive process task exists")
                            .await;
                        state.remove_output().await;
                        let error = match result {
                            Ok(Ok(())) => io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "FFmpeg exited without producing output",
                            ),
                            Ok(Err(error)) => io::Error::other(error),
                            Err(error) => {
                                io::Error::other(format!("FFmpeg process task failed: {error}"))
                            }
                        };
                        return Some((Err(error), state));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    continue;
                }
                Err(error) => return Some((Err(error), state)),
            }
        }

        let mut buffer = vec![0_u8; 64 * 1024];
        let read = state
            .file
            .as_mut()
            .expect("progressive output file is open")
            .read(&mut buffer)
            .await;
        match read {
            Ok(length) if length > 0 => {
                buffer.truncate(length);
                return Some((Ok(Bytes::from(buffer)), state));
            }
            Ok(_) => {
                if state
                    .process
                    .as_ref()
                    .is_some_and(tokio::task::JoinHandle::is_finished)
                {
                    let result = state
                        .process
                        .as_mut()
                        .expect("progressive process task exists")
                        .await;
                    state.remove_output().await;
                    match result {
                        Ok(Ok(())) => return None,
                        Ok(Err(error)) => {
                            return Some((Err(io::Error::other(error)), state));
                        }
                        Err(error) => {
                            return Some((
                                Err(io::Error::other(format!(
                                    "FFmpeg process task failed: {error}"
                                ))),
                                state,
                            ));
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(error) => return Some((Err(error), state)),
        }
    }
}
