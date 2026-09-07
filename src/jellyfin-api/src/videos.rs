use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode, header},
    response::Response,
};
use axum_extra::extract::Query;
use jellyfin_controller::{embedded_subtitle_filter_index, video_command, video_remux_command};
use jellyfin_data::BaseItemPage;
use jellyfin_model::{MediaStream, MediaStreamType};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MergeVersionsQuery {
    #[serde(
        default,
        rename = "ids",
        alias = "Ids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<Uuid>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct StreamQuery {
    // `container` is a query parameter on the extensionless official route.
    // The by-container route receives it from the path instead.
    #[serde(rename = "container", alias = "Container")]
    container: Option<String>,
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
    #[serde(rename = "params", alias = "Params")]
    _params: Option<String>,
    #[serde(rename = "tag", alias = "Tag")]
    _tag: Option<String>,
    #[serde(
        rename = "deviceProfileId",
        alias = "DeviceProfileId",
        alias = "deviceprofileid"
    )]
    _device_profile_id: Option<String>,
    #[serde(
        rename = "playSessionId",
        alias = "PlaySessionId",
        alias = "playsessionid"
    )]
    _play_session_id: Option<String>,
    #[serde(
        rename = "segmentContainer",
        alias = "SegmentContainer",
        alias = "segmentcontainer"
    )]
    _segment_container: Option<String>,
    #[serde(
        rename = "segmentLength",
        alias = "SegmentLength",
        alias = "segmentlength"
    )]
    _segment_length: Option<i32>,
    #[serde(rename = "minSegments", alias = "MinSegments", alias = "minsegments")]
    _min_segments: Option<i32>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId", alias = "deviceid")]
    _device_id: Option<String>,
    #[serde(
        rename = "enableAutoStreamCopy",
        alias = "EnableAutoStreamCopy",
        alias = "enableautostreamcopy"
    )]
    enable_auto_stream_copy: Option<bool>,
    #[serde(
        rename = "allowVideoStreamCopy",
        alias = "AllowVideoStreamCopy",
        alias = "allowvideostreamcopy"
    )]
    allow_video_stream_copy: Option<bool>,
    #[serde(
        rename = "allowAudioStreamCopy",
        alias = "AllowAudioStreamCopy",
        alias = "allowaudiostreamcopy"
    )]
    allow_audio_stream_copy: Option<bool>,
    #[serde(rename = "videoCodec", alias = "VideoCodec", alias = "videocodec")]
    video_codec: Option<String>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "videoBitRate",
        alias = "VideoBitRate",
        alias = "VideoBitrate",
        alias = "videobitrate"
    )]
    video_bitrate: Option<i64>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
        alias = "AudioBitrate",
        alias = "audiobitrate"
    )]
    audio_bitrate: Option<i64>,
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
    _level: Option<String>,
    #[serde(rename = "framerate", alias = "Framerate")]
    framerate: Option<f32>,
    #[serde(
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels",
        alias = "transcodingmaxaudiochannels"
    )]
    transcoding_max_audio_channels: Option<i32>,
    #[serde(rename = "maxWidth", alias = "MaxWidth", alias = "maxwidth")]
    max_width: Option<i32>,
    #[serde(rename = "maxHeight", alias = "MaxHeight", alias = "maxheight")]
    max_height: Option<i32>,
    #[serde(rename = "width", alias = "Width")]
    width: Option<i32>,
    #[serde(rename = "height", alias = "Height")]
    height: Option<i32>,
    #[serde(
        rename = "maxFramerate",
        alias = "MaxFramerate",
        alias = "maxframerate"
    )]
    max_framerate: Option<f32>,
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
    video_stream_index: Option<i32>,
    #[serde(
        rename = "subtitleStreamIndex",
        alias = "SubtitleStreamIndex",
        alias = "subtitlestreamindex"
    )]
    subtitle_stream_index: Option<i32>,
    #[serde(
        rename = "subtitleMethod",
        alias = "SubtitleMethod",
        alias = "subtitlemethod"
    )]
    subtitle_method: Option<String>,
    #[serde(
        rename = "maxRefFrames",
        alias = "MaxRefFrames",
        alias = "maxrefframes"
    )]
    max_ref_frames: Option<i32>,
    #[serde(
        rename = "maxVideoBitDepth",
        alias = "MaxVideoBitDepth",
        alias = "maxvideobitdepth"
    )]
    _max_video_bit_depth: Option<i32>,
    #[serde(rename = "requireAvc", alias = "RequireAvc", alias = "requireavc")]
    require_avc: Option<bool>,
    #[serde(rename = "deInterlace", alias = "DeInterlace", alias = "deinterlace")]
    de_interlace: Option<bool>,
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
    _cpu_core_limit: Option<i32>,
    #[serde(
        rename = "liveStreamId",
        alias = "LiveStreamId",
        alias = "livestreamid"
    )]
    _live_stream_id: Option<String>,
    #[serde(
        rename = "enableMpegtsM2TsMode",
        alias = "EnableMpegtsM2TsMode",
        alias = "enablempegtsm2tsmode"
    )]
    _enable_mpegts_m2_ts_mode: Option<bool>,
    #[serde(
        rename = "subtitleCodec",
        alias = "SubtitleCodec",
        alias = "subtitlecodec"
    )]
    _subtitle_codec: Option<String>,
    #[serde(
        rename = "transcodeReasons",
        alias = "TranscodeReasons",
        alias = "transcodereasons"
    )]
    _transcode_reasons: Option<String>,
    #[serde(rename = "context", alias = "Context")]
    _context: Option<String>,
    // ASP.NET binds this as a string dictionary. The progressive path does
    // not yet use its values, but model the input explicitly so the gap is
    // visible when its streaming state gains those options.
    #[serde(
        rename = "streamOptions",
        alias = "StreamOptions",
        alias = "streamoptions"
    )]
    _stream_options: Option<String>,
    #[serde(
        rename = "enableAudioVbrEncoding",
        alias = "EnableAudioVbrEncoding",
        alias = "enableaudiovbrencoding"
    )]
    _enable_audio_vbr_encoding: Option<bool>,
}

pub(crate) async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, None, query, request).await
}

pub(crate) async fn stream_with_container(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, container)): Path<(Uuid, String)>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, Some(&container), query, request).await
}

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: StreamQuery,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity =
        authentication::authenticated_identity(&state, &headers, Some(request.uri())).await?;
    let (mut requested_item, device_policy) = match identity {
        authentication::AuthenticatedIdentity::Device(authenticated) => {
            let policy: jellyfin_model::UserPolicy =
                serde_json::from_value(authenticated.user.policy.clone())
                    .map_err(|_| ApiError::Internal)?;
            (
                state
                    .user_library
                    .item(&authenticated.user, authenticated.user.id, item_id)
                    .await?,
                Some(policy),
            )
        }
        // The official default authorization handler treats API keys as an
        // unrestricted principal. Unlike a device session, they have no
        // target-user library policy to apply before opening a stream.
        authentication::AuthenticatedIdentity::ApiKey(_) => (
            state
                .base_items
                .get(item_id)
                .await?
                .ok_or(ApiError::NotFound)?,
            None,
        ),
    };
    // Device sessions are hydrated by LibraryController. API keys load the
    // unrestricted persisted row directly, so normalize official CLR aliases
    // before applying this route's supported-video-type check.
    let item_types = jellyfin_controller::ItemTypeRegistry::default();
    let item_type = item_types
        .resolve(&requested_item.item_type)
        .ok_or(ApiError::NotFound)?;
    requested_item.item_type = item_type.name().to_owned();
    if !matches!(
        requested_item.item_type.as_str(),
        "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
    ) {
        return Err(ApiError::NotFound);
    }
    let item = if let Some(media_source_id) = query
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let version_id = Uuid::parse_str(media_source_id).map_err(|_| ApiError::NotFound)?;
        if version_id == requested_item.id {
            requested_item
        } else {
            state
                .base_items
                .alternate_video_version(requested_item.id, version_id)
                .await?
                .ok_or(ApiError::NotFound)?
        }
    } else {
        requested_item
    };
    let path = jellyfin_controller::media_source_path(&item)
        .map(str::to_owned)
        .ok_or(ApiError::NotFound)?;
    if let Some(container) = requested_container {
        let actual = std::path::Path::new(&path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        if !container.eq_ignore_ascii_case(actual) {
            // Some Jellyfin clients (including VidHub) use `stream.mp4` as a
            // generic static-stream endpoint even after selecting a direct
            // play MKV source. The route suffix is not the source of truth;
            // ServeFile derives the response MIME type from the real path.
            tracing::warn!(
                %item_id,
                requested_container = container,
                actual_container = actual,
                "static video route container differs from the media source",
            );
        }
    }
    if query.static_stream.unwrap_or(false) {
        if path
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || path
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            return proxy_remote_stream(
                &state.remote_stream_client,
                &headers,
                item_id,
                &path,
                required_remote_user_agent(&item),
            )
            .await;
        }
        return crate::audio::serve_path(headers, &path, request).await;
    }

    let container = output_container(requested_container, query.container.as_deref());
    let video_codec = query
        .video_codec
        .as_deref()
        .unwrap_or_else(|| video_codec_for_container(&container));
    let audio_codec = query
        .audio_codec
        .as_deref()
        .unwrap_or_else(|| audio_codec_for_container(&container));
    let subtitle_stream_index = query
        .subtitle_stream_index
        .filter(|_| should_burn_subtitles(query.subtitle_method.as_deref()));
    let subtitle_filter_index = if let Some(index) = subtitle_stream_index {
        let streams = state
            .media_streams
            .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item.id))
            .await?;
        embedded_subtitle_filter_index(&streams, index)
    } else {
        None
    };
    let output = state.transcode_directory.join(format!(
        "{item_id}-video-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let copy_timestamps = query.copy_timestamps.unwrap_or(false);
    let mut is_local_copy_remux = if subtitle_filter_index.is_none()
        && is_local_path(&path)
        && copy_remux_has_no_transform(&query)
    {
        let streams = state
            .media_streams
            .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item.id))
            .await?;
        can_copy_remux(&query, &container, &streams, video_codec, audio_codec)
    } else {
        false
    };
    if let Some(policy) = device_policy {
        let can_encode =
            policy.enable_audio_playback_transcoding || policy.enable_video_playback_transcoding;
        if is_local_copy_remux && !policy.enable_playback_remuxing {
            is_local_copy_remux = false;
        }
        if !is_local_copy_remux && !can_encode {
            return Err(ApiError::Forbidden);
        }
    }
    let command = if is_local_copy_remux {
        video_remux_command(
            &state.ffmpeg_path,
            std::path::Path::new(&path),
            &output,
            query.audio_stream_index,
            query.video_stream_index,
            query.start_time_ticks,
            copy_timestamps,
        )
    } else {
        video_command(
            &state.ffmpeg_path,
            std::path::Path::new(&path),
            &output,
            video_codec,
            audio_codec,
            query.video_bitrate,
            query.audio_bitrate,
            query
                .audio_channels
                .or(query.max_audio_channels)
                .or(query.transcoding_max_audio_channels),
            query.audio_sample_rate,
            query.max_width.or(query.width),
            query.max_height.or(query.height),
            query.framerate.or(query.max_framerate),
            query.de_interlace.unwrap_or(false),
            query.audio_stream_index,
            query.video_stream_index,
            query.start_time_ticks,
            subtitle_filter_index,
            copy_timestamps,
        )
    };
    crate::audio::serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
    )
    .await
}

fn is_local_path(path: &str) -> bool {
    !path.contains("://")
}

fn copy_remux_has_no_transform(query: &StreamQuery) -> bool {
    query.enable_auto_stream_copy.unwrap_or(true)
        && query.allow_video_stream_copy.unwrap_or(true)
        && query.allow_audio_stream_copy.unwrap_or(true)
        && query.video_bitrate.is_none()
        && query.audio_bitrate.is_none()
        && query.audio_channels.is_none()
        && query.max_audio_channels.is_none()
        && query.transcoding_max_audio_channels.is_none()
        && query.audio_sample_rate.is_none()
        && query.max_width.is_none()
        && query.width.is_none()
        && query.max_height.is_none()
        && query.height.is_none()
        && query.framerate.is_none()
        && query.max_framerate.is_none()
        && query.de_interlace != Some(true)
        && !(query.subtitle_stream_index.is_some()
            && should_burn_subtitles(query.subtitle_method.as_deref()))
}

fn can_copy_remux(
    query: &StreamQuery,
    container: &str,
    streams: &[MediaStream],
    requested_video_codec: &str,
    requested_audio_codec: &str,
) -> bool {
    let Some(video_stream) =
        selected_stream(streams, MediaStreamType::Video, query.video_stream_index)
    else {
        return false;
    };
    let Some(video_codec) = video_stream
        .codec
        .as_deref()
        .filter(|codec| !codec.trim().is_empty())
    else {
        return false;
    };
    let Some(audio_codec) =
        selected_stream_codec(streams, MediaStreamType::Audio, query.audio_stream_index)
    else {
        return false;
    };
    !(query.require_avc == Some(true)
        && video_codec.eq_ignore_ascii_case("h264")
        && video_stream.is_avc == Some(false))
        && !query.max_ref_frames.is_some_and(|maximum| {
            video_stream
                .ref_frames
                .is_some_and(|actual| actual > maximum)
        })
        && codecs_match(video_codec, requested_video_codec)
        && codecs_match(audio_codec, requested_audio_codec)
        && copy_remux_container_supports(container, video_codec, audio_codec)
}

fn selected_stream_codec(
    streams: &[MediaStream],
    stream_type: MediaStreamType,
    requested_index: Option<i32>,
) -> Option<&str> {
    selected_stream(streams, stream_type, requested_index)
        .and_then(|stream| stream.codec.as_deref())
        .filter(|codec| !codec.trim().is_empty())
}

fn selected_stream(
    streams: &[MediaStream],
    stream_type: MediaStreamType,
    requested_index: Option<i32>,
) -> Option<&MediaStream> {
    streams.iter().find(|stream| {
        stream.stream_type == stream_type
            && requested_index.is_none_or(|index| stream.index == index)
    })
}

fn codecs_match(actual: &str, requested: &str) -> bool {
    actual.trim().eq_ignore_ascii_case(requested.trim())
}

fn copy_remux_container_supports(container: &str, video_codec: &str, audio_codec: &str) -> bool {
    let container = container.trim().to_ascii_lowercase();
    let video_codec = video_codec.trim().to_ascii_lowercase();
    let audio_codec = audio_codec.trim().to_ascii_lowercase();
    match container.as_str() {
        // Matroska accepts the broadest practical set of already-probed
        // FFmpeg codecs, so it is the safe fallback for clients that support
        // it. Keep MP4/WebM conservative: a failed mux after advertising a
        // direct stream is worse than falling back to an encode.
        "mkv" | "matroska" => true,
        "mp4" | "m4v" | "mov" => {
            matches!(
                video_codec.as_str(),
                "h264" | "hevc" | "h265" | "av1" | "mpeg4"
            ) && matches!(
                audio_codec.as_str(),
                "aac" | "ac3" | "eac3" | "mp3" | "alac"
            )
        }
        "ts" | "mpegts" => {
            matches!(
                video_codec.as_str(),
                "h264" | "hevc" | "h265" | "mpeg2video"
            ) && matches!(audio_codec.as_str(), "aac" | "ac3" | "eac3" | "mp3")
        }
        "webm" => {
            matches!(video_codec.as_str(), "vp8" | "vp9" | "av1")
                && matches!(audio_codec.as_str(), "opus" | "vorbis")
        }
        _ => false,
    }
}

fn should_burn_subtitles(method: Option<&str>) -> bool {
    method.is_none_or(|method| {
        let method = method.trim();
        method.eq_ignore_ascii_case("Encode") || method == "0"
    })
}

fn video_codec_for_container(container: &str) -> &str {
    match container {
        "webm" => "vp9",
        _ => "h264",
    }
}

fn audio_codec_for_container(container: &str) -> &str {
    match container {
        "webm" => "opus",
        _ => "aac",
    }
}

fn output_container(requested_container: Option<&str>, query_container: Option<&str>) -> String {
    requested_container
        .or(query_container)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".to_owned())
}

pub(crate) fn required_remote_user_agent(
    item: &jellyfin_data::entities::base_item::Model,
) -> Option<&str> {
    let headers = item
        .data
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|data| {
            [
                "RequiredHttpHeaders",
                "requiredHttpHeaders",
                "required_http_headers",
            ]
            .iter()
            .find_map(|key| data.get(*key))
        })
        .and_then(serde_json::Value::as_object)?;
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("User-Agent"))
        .and_then(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn proxy_remote_stream(
    client: &reqwest::Client,
    client_headers: &HeaderMap,
    item_id: Uuid,
    path: &str,
    user_agent: Option<&str>,
) -> Result<Response, ApiError> {
    let mut request = client.get(path);
    if let Some(user_agent) = user_agent {
        request = request.header(header::USER_AGENT, user_agent);
    }
    if let Some(range) = client_headers.get(header::RANGE) {
        request = request.header(header::RANGE, range);
    }
    let upstream = request.send().await.map_err(|error| {
        tracing::warn!(
            %item_id,
            timeout = error.is_timeout(),
            connect = error.is_connect(),
            "remote video stream request failed",
        );
        ApiError::UpstreamUnavailable
    })?;
    let status = upstream.status();
    let mut response = Response::builder().status(status);
    let headers = response.headers_mut().ok_or(ApiError::Internal)?;
    for name in [
        header::CONTENT_RANGE,
        header::CONTENT_LENGTH,
        header::CONTENT_TYPE,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/octet-stream"),
        );
    }
    let accept_ranges = upstream
        .headers()
        .get(header::ACCEPT_RANGES)
        .cloned()
        .unwrap_or_else(|| {
            header::HeaderValue::from_static(if status == StatusCode::PARTIAL_CONTENT {
                "bytes"
            } else {
                "none"
            })
        });
    headers.insert(header::ACCEPT_RANGES, accept_ranges);
    tracing::info!(
        %item_id,
        %status,
        range_requested = client_headers.contains_key(header::RANGE),
        "proxying remote video stream",
    );
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|_| ApiError::Internal)
}

pub(crate) async fn delete_alternate_sources(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .videos
        .clear_alternate_sources(&authenticated.user, item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn merge_versions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MergeVersionsQuery>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .videos
        .merge_versions(&authenticated.user, &query.ids)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn additional_parts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<user_library::UserIdQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let items = state
        .user_library
        .additional_parts(&authenticated.user, target_user_id, item_id)
        .await?;
    let total_record_count = u64::try_from(items.len()).unwrap_or(u64::MAX);
    Ok(Json(
        crate::items::page_to_dto_all_fields(
            state.as_ref(),
            BaseItemPage {
                items,
                total_record_count,
                start_index: 0,
            },
            target_user_id,
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        time::Duration,
    };

    use axum::{
        body::to_bytes,
        http::{Uri, header},
    };
    use axum_extra::extract::Query;

    use super::*;

    #[test]
    fn video_stream_binds_android_progressive_parameters() {
        // These names deliberately mix the canonical SDK casing, PascalCase,
        // and lower-case legacy spelling accepted by ASP.NET model binding.
        let uri: Uri = "/videos/item/stream.mp4?container=webm&static=false&params=client%3Dandroid&Tag=etag&deviceprofileid=profile&PlaySessionId=play-session&segmentcontainer=ts&SegmentLength=6&minsegments=2&MediaSourceId=alternate&deviceid=device&enableautostreamcopy=true&AllowVideoStreamCopy=false&allowaudiostreamcopy=true&videoCodec=h264&audioCodec=aac&VideoBitrate=2000000&AudioBitrate=128000&audioSampleRate=48000&MaxAudioBitDepth=24&audioChannels=2&maxAudioChannels=6&Profile=high&Level=4.1&framerate=24&width=1280&Height=720&MaxFramerate=23.976&audioStreamIndex=2&videoStreamIndex=0&subtitleStreamIndex=3&subtitleMethod=Encode&MaxRefFrames=4&maxvideobitdepth=10&RequireAvc=true&deinterlace=true&requireNonAnamorphic=true&startTimeTicks=10000&CopyTimestamps=true&cpuCoreLimit=2&liveStreamId=live&enableMpegtsM2TsMode=true&subtitleCodec=srt&transcodeReasons=ContainerNotSupported&context=Streaming&streamoptions=quality%3Dhigh&enableAudioVbrEncoding=false"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.container.as_deref(), Some("webm"));
        assert!(!query.static_stream.unwrap());
        assert_eq!(query._params.as_deref(), Some("client=android"));
        assert_eq!(query._tag.as_deref(), Some("etag"));
        assert_eq!(query._device_profile_id.as_deref(), Some("profile"));
        assert_eq!(query._play_session_id.as_deref(), Some("play-session"));
        assert_eq!(query._segment_container.as_deref(), Some("ts"));
        assert_eq!(query._segment_length, Some(6));
        assert_eq!(query._min_segments, Some(2));
        assert_eq!(query.media_source_id.as_deref(), Some("alternate"));
        assert_eq!(query._device_id.as_deref(), Some("device"));
        assert_eq!(query.enable_auto_stream_copy, Some(true));
        assert_eq!(query.allow_video_stream_copy, Some(false));
        assert_eq!(query.allow_audio_stream_copy, Some(true));
        assert_eq!(query.video_codec.as_deref(), Some("h264"));
        assert_eq!(query.audio_codec.as_deref(), Some("aac"));
        assert_eq!(query.video_bitrate, Some(2_000_000));
        assert_eq!(query.audio_bitrate, Some(128_000));
        assert_eq!(query.audio_sample_rate, Some(48_000));
        assert_eq!(query._max_audio_bit_depth, Some(24));
        assert_eq!(query.audio_channels, Some(2));
        assert_eq!(query.max_audio_channels, Some(6));
        assert_eq!(query._profile.as_deref(), Some("high"));
        assert_eq!(query._level.as_deref(), Some("4.1"));
        assert_eq!(query.framerate, Some(24.0));
        assert_eq!(query.width, Some(1280));
        assert_eq!(query.height, Some(720));
        assert_eq!(query.max_framerate, Some(23.976));
        assert_eq!(query.audio_stream_index, Some(2));
        assert_eq!(query.video_stream_index, Some(0));
        assert_eq!(query.subtitle_stream_index, Some(3));
        assert_eq!(query.subtitle_method.as_deref(), Some("Encode"));
        assert_eq!(query.max_ref_frames, Some(4));
        assert_eq!(query._max_video_bit_depth, Some(10));
        assert_eq!(query.require_avc, Some(true));
        assert_eq!(query.de_interlace, Some(true));
        assert_eq!(query._require_non_anamorphic, Some(true));
        assert_eq!(query.start_time_ticks, Some(10_000));
        assert_eq!(query.copy_timestamps, Some(true));
        assert_eq!(query._cpu_core_limit, Some(2));
        assert_eq!(query._live_stream_id.as_deref(), Some("live"));
        assert_eq!(query._enable_mpegts_m2_ts_mode, Some(true));
        assert_eq!(query._subtitle_codec.as_deref(), Some("srt"));
        assert_eq!(
            query._transcode_reasons.as_deref(),
            Some("ContainerNotSupported")
        );
        assert_eq!(query._context.as_deref(), Some("Streaming"));
        assert_eq!(query._stream_options.as_deref(), Some("quality=high"));
        assert_eq!(query._enable_audio_vbr_encoding, Some(false));
        assert_eq!(video_codec_for_container("mp4"), "h264");
        assert_eq!(audio_codec_for_container("webm"), "opus");
        assert!(should_burn_subtitles(Some("Encode")));
        assert!(should_burn_subtitles(Some("0")));
        assert!(!should_burn_subtitles(Some("External")));
    }

    #[test]
    fn video_stream_binds_lowercase_container_and_framerate() {
        let uri: Uri = "/videos/item/stream?container=.WEBM&framerate=25"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.container.as_deref(), Some(".WEBM"));
        assert_eq!(query.framerate, Some(25.0));
        assert_eq!(
            output_container(None, query.container.as_deref()),
            "webm",
            "extensionless routes use the official query container"
        );
        assert_eq!(
            output_container(Some("mkv"), query.container.as_deref()),
            "mkv",
            "the by-container route remains authoritative"
        );
    }

    #[test]
    fn local_copy_remux_requires_matching_codecs_and_no_transform() {
        let streams = vec![
            MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("h264".to_owned()),
                ..MediaStream::default()
            },
            MediaStream {
                index: 1,
                stream_type: MediaStreamType::Audio,
                codec: Some("aac".to_owned()),
                ..MediaStream::default()
            },
        ];
        let query = StreamQuery {
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            ..StreamQuery::default()
        };

        assert!(copy_remux_has_no_transform(&query));
        assert!(can_copy_remux(&query, "mp4", &streams, "h264", "aac"));

        let non_avc_streams = vec![
            MediaStream {
                is_avc: Some(false),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        let require_avc = StreamQuery {
            require_avc: Some(true),
            ..query.clone()
        };
        assert!(!can_copy_remux(
            &require_avc,
            "mp4",
            &non_avc_streams,
            "h264",
            "aac",
        ));

        let too_many_ref_frames = StreamQuery {
            max_ref_frames: Some(3),
            ..query.clone()
        };
        let high_ref_streams = vec![
            MediaStream {
                ref_frames: Some(4),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        assert!(!can_copy_remux(
            &too_many_ref_frames,
            "mp4",
            &high_ref_streams,
            "h264",
            "aac",
        ));

        let resized = StreamQuery {
            max_width: Some(1280),
            ..query
        };
        assert!(!copy_remux_has_no_transform(&resized));
        let no_auto_copy = StreamQuery {
            enable_auto_stream_copy: Some(false),
            ..StreamQuery::default()
        };
        assert!(!copy_remux_has_no_transform(&no_auto_copy));
        assert!(!can_copy_remux(
            &StreamQuery {
                video_codec: Some("hevc".to_owned()),
                audio_codec: Some("aac".to_owned()),
                ..StreamQuery::default()
            },
            "mp4",
            &streams,
            "hevc",
            "aac",
        ));
        assert!(!is_local_path("https://media.example/video.mkv"));
        assert!(!is_local_path("rtsp://media.example/video.mkv"));
        assert!(is_local_path("/library/video.mkv"));
    }

    #[tokio::test]
    async fn remote_stream_forwards_ranges_without_buffering_the_upstream_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock upstream listener");
        let address = listener.local_addr().expect("mock upstream address");
        let (release_sender, release_receiver) = mpsc::channel();
        let upstream = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("upstream request");
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("upstream read timeout");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).expect("upstream request bytes");
                assert_ne!(read, 0, "request ended before its headers");
                request.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Type: video/x-matroska\r\nContent-Length: 10\r\nContent-Range: bytes 20-29/100\r\nConnection: close\r\n\r\nhello",
                )
                .expect("first upstream chunk");
            socket.flush().expect("flush upstream headers");
            release_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("proxy returned before the complete body");
            socket.write_all(b"world").expect("last upstream chunk");
            String::from_utf8(request).expect("HTTP request text")
        });

        let mut headers = HeaderMap::new();
        headers.insert(
            header::RANGE,
            header::HeaderValue::from_static("bytes=20-29"),
        );
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            proxy_remote_stream(
                &reqwest::Client::new(),
                &headers,
                Uuid::new_v4(),
                &format!("http://{address}/signed/video.mkv?token=secret"),
                Some("Jellyfin Remote Stream Test"),
            ),
        )
        .await
        .expect("proxy must return after upstream headers")
        .expect("remote response");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 20-29/100");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/x-matroska");
        release_sender.send(()).expect("release upstream body");
        assert_eq!(
            to_bytes(response.into_body(), 10)
                .await
                .expect("proxied body"),
            "helloworld"
        );
        let request = upstream.join().expect("mock upstream thread");
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("range: bytes=20-29")),
            "{request}"
        );
        assert!(
            request.lines().any(|line| {
                line.eq_ignore_ascii_case("user-agent: Jellyfin Remote Stream Test")
            }),
            "{request}"
        );
    }

    #[tokio::test]
    async fn unavailable_remote_stream_maps_to_bad_gateway() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("unused listener");
        let address = listener.local_addr().expect("unused listener address");
        drop(listener);
        let error = proxy_remote_stream(
            &reqwest::Client::new(),
            &HeaderMap::new(),
            Uuid::new_v4(),
            &format!("http://{address}/video.mkv"),
            None,
        )
        .await
        .expect_err("closed upstream port must fail");
        assert!(matches!(error, ApiError::UpstreamUnavailable));
    }
}
