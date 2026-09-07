use std::{io, path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request},
    response::Response,
};
use bytes::Bytes;
use futures_util::stream;
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use jellyfin_controller::{FfmpegCommand, audio_command};

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamQuery {
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
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
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels",
        alias = "transcodingmaxaudiochannels"
    )]
    transcoding_max_audio_channels: Option<i32>,
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

pub(crate) async fn universal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UniversalQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(identity.user.id);
    if target_user_id != identity.user.id && !identity.user.is_administrator {
        return Err(ApiError::Forbidden);
    }
    let item = state
        .library_controller
        .item(&identity.user, target_user_id, item_id)
        .await?;
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let path = item.path.as_deref().ok_or(ApiError::NotFound)?;
    let actual_container = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    let supports_direct = query.container.iter().any(|profile| {
        profile
            .split('|')
            .next()
            .is_some_and(|container| container.eq_ignore_ascii_case(actual_container))
    });
    let requires_transcode = query.audio_codec.is_some()
        || query.max_audio_channels.is_some()
        || query.audio_stream_index.is_some()
        || query.max_streaming_bitrate.is_some()
        || query.start_time_ticks.is_some_and(|ticks| ticks != 0)
        || query.transcoding_container.is_some();
    if supports_direct && !requires_transcode {
        return serve_path(headers, path, request).await;
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
    let command = audio_command(
        &state.ffmpeg_path,
        std::path::Path::new(path),
        &output,
        &codec,
        query.max_streaming_bitrate,
        query.max_audio_channels,
        None,
        query.audio_stream_index,
        query.start_time_ticks,
        false,
    );
    serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
    )
    .await
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

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: StreamQuery,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let requested_item = state
        .library_controller
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    if requested_item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let item = if let Some(media_source_id) = query
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
    let path = item
        .path
        .filter(|path| !path.is_empty())
        .ok_or(ApiError::NotFound)?;
    if let Some(container) = requested_container {
        let actual = std::path::Path::new(&path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        if !container.eq_ignore_ascii_case(actual) {
            return Err(ApiError::UnsupportedMediaType);
        }
    }
    if query.static_stream.unwrap_or(false) {
        return serve_path(headers, &path, request).await;
    }

    let codec = query
        .audio_codec
        .as_deref()
        .unwrap_or_else(|| codec_for_container(requested_container.unwrap_or("m4a")));
    let container = requested_container
        .filter(|container| !container.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| audio_container(codec).to_owned());
    let output = state.transcode_directory.join(format!(
        "{item_id}-audio-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let command = audio_command(
        &state.ffmpeg_path,
        std::path::Path::new(&path),
        &output,
        codec,
        query.audio_bitrate,
        query
            .audio_channels
            .or(query.max_audio_channels)
            .or(query.transcoding_max_audio_channels),
        query.audio_sample_rate,
        query.audio_stream_index,
        query.start_time_ticks,
        query.copy_timestamps.unwrap_or(false),
    );
    serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;
    use axum_extra::extract::Query;

    use super::StreamQuery;

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
        let uri: Uri = "/audio/item/stream?static=false&audioCodec=mp3&AudioBitrate=192000&audioSampleRate=44100&maxAudioChannels=2&audioStreamIndex=1&startTimeTicks=10000&CopyTimestamps=true"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert!(!query.static_stream.unwrap());
        assert_eq!(query.audio_codec.as_deref(), Some("mp3"));
        assert_eq!(query.audio_bitrate, Some(192000));
        assert_eq!(query.audio_sample_rate, Some(44100));
        assert_eq!(query.max_audio_channels, Some(2));
        assert_eq!(query.audio_stream_index, Some(1));
        assert_eq!(query.start_time_ticks, Some(10000));
        assert_eq!(query.copy_timestamps, Some(true));
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

/// Starts a progressive FFmpeg job and streams the output as it grows.
///
/// Jellyfin's progressive endpoints return the response before FFmpeg has
/// completed. Android's ExoPlayer relies on that behavior for long files.
pub(crate) async fn serve_transcoded_path(
    command: FfmpegCommand,
    output_path: &str,
    is_head: bool,
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
    let state = ProgressiveTranscodeState {
        child,
        output_path: PathBuf::from(output_path),
        file: None,
    };
    let body = Body::from_stream(stream::unfold(state, next_transcode_chunk));
    response.body(body).map_err(|_| ApiError::Internal)
}

struct ProgressiveTranscodeState {
    child: tokio::process::Child,
    output_path: PathBuf,
    file: Option<tokio::fs::File>,
}

async fn next_transcode_chunk(
    mut state: ProgressiveTranscodeState,
) -> Option<(Result<Bytes, io::Error>, ProgressiveTranscodeState)> {
    loop {
        if state.file.is_none() {
            match tokio::fs::File::open(&state.output_path).await {
                Ok(file) => state.file = Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if let Some(status) = state.child.try_wait().ok().flatten() {
                        return Some((
                            Err(if status.success() {
                                io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "FFmpeg exited without producing output",
                                )
                            } else {
                                io::Error::other(format!("FFmpeg exited with {status}"))
                            }),
                            state,
                        ));
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
            Ok(_) => match state.child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    let _ = tokio::fs::remove_file(&state.output_path).await;
                    return None;
                }
                Ok(Some(status)) => {
                    let _ = tokio::fs::remove_file(&state.output_path).await;
                    return Some((
                        Err(io::Error::other(format!("FFmpeg exited with {status}"))),
                        state,
                    ));
                }
                Ok(None) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => return Some((Err(error), state)),
            },
            Err(error) => return Some((Err(error), state)),
        }
    }
}
