use std::{path::Path as FilePath, sync::Arc};

use axum::{
    body::Body,
    extract::{OriginalUri, Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use jellyfin_extensions::PathHelper;
use jellyfin_model::MimeTypes;
use serde::Deserialize;
use tokio::fs;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

pub(crate) async fn audio(
    State(state): State<Arc<AppState>>,
    Path((_item_id, legacy_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (segment_id, extension) = parse_audio_path(&legacy_path)?;
    let path = resolve_transcode_file(
        &state.transcode_directory,
        &format!("{segment_id}.{extension}"),
    )?;
    serve_file(path, &headers).await
}

pub(crate) async fn audio_master_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    serve_authenticated_playlist(&state, &headers, &uri, "master.m3u8").await
}

pub(crate) async fn audio_main_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    serve_authenticated_playlist(&state, &headers, &uri, "main.m3u8").await
}

pub(crate) async fn audio_hls1_segment(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((_item_id, playlist_id, segment_file)): Path<(Uuid, String, String)>,
    Query(query): Query<DynamicSegmentQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (segment_id, container) = parse_hls1_segment_file(&segment_file)?;
    serve_authenticated_hls1_segment(
        &state,
        &headers,
        &uri,
        &playlist_id,
        segment_id,
        container,
        query,
    )
    .await
}

pub(crate) async fn video(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((_item_id, legacy_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some((playlist_id, stream_file)) = parse_stream_path(&legacy_path) {
        authorization::require_default(&state, &headers, &uri).await?;
        return serve_playlist(&state, &headers, playlist_id, stream_file).await;
    }

    serve_video_segment(&state, &headers, &legacy_path).await
}

pub(crate) async fn video_live_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    serve_authenticated_playlist(&state, &headers, &uri, "live.m3u8").await
}

pub(crate) async fn video_master_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    serve_authenticated_playlist(&state, &headers, &uri, "master.m3u8").await
}

pub(crate) async fn video_main_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    serve_authenticated_playlist(&state, &headers, &uri, "main.m3u8").await
}

pub(crate) async fn video_hls1_segment(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path((_item_id, playlist_id, segment_file)): Path<(Uuid, String, String)>,
    Query(query): Query<DynamicSegmentQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (segment_id, container) = parse_hls1_segment_file(&segment_file)?;
    serve_authenticated_hls1_segment(
        &state,
        &headers,
        &uri,
        &playlist_id,
        segment_id,
        container,
        query,
    )
    .await
}

#[derive(Debug, Deserialize)]
pub(crate) struct DynamicSegmentQuery {
    #[serde(rename = "runtimeTicks", alias = "RuntimeTicks")]
    runtime_ticks: i64,
    #[serde(
        rename = "actualSegmentLengthTicks",
        alias = "ActualSegmentLengthTicks"
    )]
    actual_segment_length_ticks: i64,
    #[serde(rename = "startTimeTicks", alias = "StartTimeTicks")]
    start_time_ticks: Option<i64>,
}

fn parse_audio_path(path: &str) -> Result<(&str, &str), ApiError> {
    for extension in ["mp3", "aac"] {
        let suffix = format!("/stream.{extension}");
        if let Some(segment_id) = strip_suffix_ascii_case(path, &suffix)
            && !segment_id.is_empty()
        {
            let request_extension = &path[path.len() - extension.len()..];
            return Ok((segment_id, request_extension));
        }
    }
    Err(ApiError::InvalidRequest)
}

fn parse_stream_path(path: &str) -> Option<(&str, &str)> {
    let (playlist_id, stream_file) = path.rsplit_once('/')?;
    stream_file
        .get(.."stream.".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("stream."))
        .then_some((playlist_id, stream_file))
}

fn parse_hls1_segment_file(path: &str) -> Result<(i32, &str), ApiError> {
    let (segment_id, container) = path.rsplit_once('.').ok_or(ApiError::InvalidRequest)?;
    let segment_id = segment_id
        .parse::<i32>()
        .map_err(|_| ApiError::InvalidRequest)?;
    Ok((segment_id, container))
}

async fn serve_playlist(
    state: &AppState,
    headers: &HeaderMap,
    playlist_id: &str,
    stream_file: &str,
) -> Result<Response, ApiError> {
    let extension = stream_file
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| extension.eq_ignore_ascii_case("m3u8"))
        .ok_or(ApiError::InvalidRequest)?;
    if playlist_id.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    let path = resolve_transcode_file(
        &state.transcode_directory,
        &format!("{playlist_id}.{extension}"),
    )?;
    serve_file(path, headers).await
}

async fn serve_video_segment(
    state: &AppState,
    headers: &HeaderMap,
    legacy_path: &str,
) -> Result<Response, ApiError> {
    let (playlist_id, segment_file) = legacy_path
        .split_once('/')
        .filter(|(playlist_id, segment_file)| !playlist_id.is_empty() && !segment_file.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let (_, segment_container) = segment_file
        .rsplit_once('.')
        .filter(|(segment_id, segment_container)| {
            !segment_id.is_empty() && !segment_container.is_empty()
        })
        .ok_or(ApiError::InvalidRequest)?;

    // Validate the caller-controlled segment before touching the transcode directory.
    let segment_path = resolve_transcode_file(&state.transcode_directory, segment_file)?;
    if find_playlist(&state.transcode_directory, playlist_id, segment_container)
        .await?
        .is_none()
    {
        return Ok((StatusCode::NOT_FOUND, "Hls segment not found.").into_response());
    }

    serve_file(segment_path, headers).await
}

async fn serve_authenticated_playlist(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    filename: &str,
) -> Result<Response, ApiError> {
    authorization::require_default(state, headers, uri).await?;
    let path = resolve_transcode_file(&state.transcode_directory, filename)?;
    serve_file(path, headers).await
}

async fn serve_authenticated_hls1_segment(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    playlist_id: &str,
    segment_id: i32,
    container: &str,
    query: DynamicSegmentQuery,
) -> Result<Response, ApiError> {
    authorization::require_default(state, headers, uri).await?;
    if query.runtime_ticks < 0
        || query.actual_segment_length_ticks <= 0
        || query.start_time_ticks.is_some_and(|ticks| ticks > 0)
        || !is_hls_container(container)
    {
        return Err(ApiError::InvalidRequest);
    }

    let path = resolve_transcode_file(
        &state.transcode_directory,
        &format!("{playlist_id}{segment_id}.{container}"),
    )?;
    serve_file(path, headers).await
}

fn resolve_transcode_file(root: &FilePath, filename: &str) -> Result<std::path::PathBuf, ApiError> {
    let relative = FilePath::new(filename);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::ParentDir
            )
        })
    {
        return Err(ApiError::InvalidRequest);
    }

    let candidate = root.join(relative);
    if !PathHelper::is_contained_in(root, &candidate).map_err(|_| ApiError::Internal)? {
        return Err(ApiError::InvalidRequest);
    }
    Ok(candidate)
}

fn is_hls_container(container: &str) -> bool {
    !container.is_empty() && container.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

async fn find_playlist(
    root: &FilePath,
    playlist_id: &str,
    segment_container: &str,
) -> Result<Option<std::path::PathBuf>, ApiError> {
    let mut entries = match fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ApiError::Internal),
    };
    let playlist_id = playlist_id.to_ascii_lowercase();

    while let Some(entry) = entries.next_entry().await.map_err(|_| ApiError::Internal)? {
        if !entry
            .file_type()
            .await
            .map_err(|_| ApiError::Internal)?
            .is_file()
        {
            continue;
        }
        let path = entry.path();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let supported_extension = extension.eq_ignore_ascii_case(segment_container)
            || extension.eq_ignore_ascii_case("m3u8");
        let basename_matches = path
            .file_name()
            .and_then(|basename| basename.to_str())
            .is_some_and(|basename| basename.to_ascii_lowercase().contains(&playlist_id));
        if supported_extension && basename_matches {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

async fn serve_file(path: std::path::PathBuf, headers: &HeaderMap) -> Result<Response, ApiError> {
    let mut request = Request::builder()
        .method("GET")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    if let Some(value) = headers.get(header::RANGE)
        && if_range_allows(&path, headers.get(header::IF_RANGE)).await
    {
        request.headers_mut().insert(header::RANGE, value.clone());
    }
    if let Some(value) = headers.get(header::IF_RANGE) {
        request
            .headers_mut()
            .insert(header::IF_RANGE, value.clone());
    }
    let response = match ServeFile::new(&path)
        .with_buf_chunk_size(64 * 1024)
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let mut response = response.map(Body::new);
    if response.status().is_success() {
        let mime_type =
            MimeTypes::get_mime_type(&path.to_string_lossy()).map_err(|_| ApiError::Internal)?;
        let content_type = HeaderValue::from_str(&mime_type).map_err(|_| ApiError::Internal)?;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(response)
}

async fn if_range_allows(path: &FilePath, if_range: Option<&HeaderValue>) -> bool {
    let Some(if_range) = if_range else {
        return true;
    };
    let Ok(if_range) = if_range.to_str() else {
        return false;
    };

    // ServeFile does not emit representation tags, so an entity-tag validator
    // cannot strongly match this response. HTTP-date validators compare at the
    // whole-second precision used by Last-Modified.
    let Ok(if_range_date) = DateTime::parse_from_rfc2822(if_range) else {
        return false;
    };
    let Ok(metadata) = fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    DateTime::<Utc>::from(modified).timestamp() <= if_range_date.timestamp()
}

fn strip_suffix_ascii_case<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    let split = value.len().checked_sub(suffix.len())?;
    let tail = value.get(split..)?;
    let head = value.get(..split)?;
    tail.eq_ignore_ascii_case(suffix).then_some(head)
}
