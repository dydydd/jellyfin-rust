use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request},
    response::Response,
};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use jellyfin_controller::{audio_command, run_ffmpeg};

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamQuery {
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UniversalQuery {
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    container: Vec<String>,
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "audioCodec", alias = "AudioCodec")]
    audio_codec: Option<String>,
    #[serde(rename = "maxAudioChannels", alias = "MaxAudioChannels")]
    max_audio_channels: Option<i32>,
    #[serde(rename = "maxStreamingBitrate", alias = "MaxStreamingBitrate")]
    max_streaming_bitrate: Option<i64>,
    #[serde(rename = "startTimeTicks", alias = "StartTimeTicks")]
    start_time_ticks: Option<i64>,
    #[serde(rename = "transcodingContainer", alias = "TranscodingContainer")]
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
        || query.max_streaming_bitrate.is_some()
        || query.start_time_ticks.is_some_and(|ticks| ticks != 0)
        || query.transcoding_container.is_some();
    if supports_direct && !requires_transcode {
        return serve_path(&headers, path, request).await;
    }

    let codec = query.audio_codec.as_deref().unwrap_or("aac").to_owned();
    let container = query.transcoding_container.as_deref().map_or_else(
        || audio_container(&codec).to_owned(),
        |container| container.trim_start_matches('.').to_owned(),
    );
    let output = state
        .transcode_directory
        .join(format!("{item_id}-{codec}.{container}"));
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
        query.start_time_ticks,
    );
    let job = state.transcode_jobs.register(output.to_string_lossy());
    run_ffmpeg(&command, &job)
        .await
        .map_err(|_| ApiError::UnsupportedMediaType)?;
    serve_path(&headers, &output.to_string_lossy(), request).await
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

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: StreamQuery,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let item = state
        .library_controller
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
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
    if !query.static_stream.unwrap_or(true) {
        return Err(ApiError::UnsupportedMediaType);
    }
    serve_path(&headers, &path, request).await
}

pub(crate) async fn serve_path(
    headers: &HeaderMap,
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
        if let Some(value) = headers.get(&name) {
            request.headers_mut().insert(name, value.clone());
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
