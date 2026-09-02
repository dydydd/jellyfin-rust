use std::{path::Path as FilePath, sync::Arc};

use axum::{
    body::Body,
    extract::{OriginalUri, Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use jellyfin_controller::{
    HlsSegmentSettings, HlsVariant, TranscodeTarget, build_main_playlist,
    build_variant_master_playlist, hls_command, hls_job_id, run_ffmpeg, wait_for_segment,
};
use jellyfin_extensions::PathHelper;
use jellyfin_model::MimeTypes;
use serde::Deserialize;
use tokio::fs;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TranscodeQuery {
    #[serde(rename = "jobId", alias = "JobId")]
    job_id: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId")]
    device_id: Option<String>,
    #[serde(rename = "playSessionId", alias = "PlaySessionId")]
    play_session_id: Option<String>,
    #[serde(rename = "mediaSourceId", alias = "MediaSourceId")]
    media_source_id: Option<String>,
    #[serde(rename = "videoCodec", alias = "VideoCodec")]
    video_codec: Option<String>,
    #[serde(rename = "audioCodec", alias = "AudioCodec")]
    audio_codec: Option<String>,
    #[serde(rename = "videoBitrate", alias = "VideoBitrate")]
    video_bitrate: Option<i64>,
    #[serde(rename = "audioBitrate", alias = "AudioBitrate")]
    audio_bitrate: Option<i64>,
    #[serde(rename = "audioStreamIndex", alias = "AudioStreamIndex")]
    audio_stream_index: Option<i32>,
    #[serde(rename = "subtitleStreamIndex", alias = "SubtitleStreamIndex")]
    subtitle_stream_index: Option<i32>,
    #[serde(rename = "burnSubtitles", alias = "BurnSubtitles")]
    burn_subtitles: Option<bool>,
    #[serde(rename = "audioNormalize", alias = "AudioNormalize")]
    audio_normalize: Option<bool>,
    #[serde(rename = "enableHDRToneMapping", alias = "EnableHDRToneMapping")]
    enable_hdr_tone_mapping: Option<bool>,
    #[serde(rename = "hwaccel", alias = "Hwaccel")]
    hwaccel: Option<String>,
    #[serde(rename = "audioSampleRate", alias = "AudioSampleRate")]
    audio_sample_rate: Option<i32>,
    #[serde(rename = "maxWidth", alias = "MaxWidth")]
    max_width: Option<i32>,
    #[serde(rename = "maxHeight", alias = "MaxHeight")]
    max_height: Option<i32>,
    #[serde(rename = "maxFramerate", alias = "MaxFramerate")]
    max_framerate: Option<f32>,
    #[serde(
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels"
    )]
    max_audio_channels: Option<i32>,
    #[serde(rename = "segmentContainer", alias = "SegmentContainer")]
    segment_container: Option<String>,
    #[serde(rename = "segmentLength", alias = "SegmentLength")]
    segment_length: Option<i32>,
    #[serde(rename = "minSegments", alias = "MinSegments")]
    min_segments: Option<i32>,
    #[serde(rename = "startTimeTicks", alias = "StartTimeTicks")]
    start_time_ticks: Option<i64>,
}

impl TranscodeQuery {
    fn has_transcode_parameters(&self) -> bool {
        self.video_codec.is_some()
            || self.audio_codec.is_some()
            || self.video_bitrate.is_some()
            || self.audio_bitrate.is_some()
            || self.audio_sample_rate.is_some()
            || self.subtitle_stream_index.is_some()
            || self.burn_subtitles == Some(true)
            || self.audio_normalize == Some(true)
            || self.enable_hdr_tone_mapping == Some(true)
            || self.hwaccel.is_some()
            || self.max_width.is_some()
            || self.max_height.is_some()
            || self.max_framerate.is_some()
            || self.max_audio_channels.is_some()
            || self.segment_container.is_some()
            || self.segment_length.is_some()
            || self.min_segments.is_some()
            || self.start_time_ticks.is_some()
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActiveEncodingQuery {
    #[serde(rename = "deviceId", alias = "DeviceId")]
    device_id: String,
    #[serde(rename = "playSessionId", alias = "PlaySessionId")]
    play_session_id: String,
}

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
    query: Query<TranscodeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    ensure_master_playlist(&state, &headers, &uri, &query, &identity).await
}

pub(crate) async fn audio_main_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    query: Query<TranscodeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    ensure_main_playlist(&state, &headers, &uri, &query, &identity).await
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
    query: Query<TranscodeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    ensure_master_playlist(&state, &headers, &uri, &query, &identity).await
}

pub(crate) async fn video_main_playlist(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Path(_item_id): Path<Uuid>,
    query: Query<TranscodeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    ensure_main_playlist(&state, &headers, &uri, &query, &identity).await
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

pub(crate) async fn stop_active_encoding(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<ActiveEncodingQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    if query.device_id.trim().is_empty() || query.play_session_id.trim().is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    let stopped = state
        .transcode_jobs
        .stop_for_session(&query.device_id, &query.play_session_id)
        .await;
    for job_id in &stopped {
        cleanup_transcode_job(&state.transcode_directory, job_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_master_playlist(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    query: &TranscodeQuery,
    identity: &crate::authentication::AuthenticatedIdentity,
) -> Result<Response, ApiError> {
    if query.job_id.is_none() && !query.has_transcode_parameters() {
        let static_path = resolve_transcode_file(&state.transcode_directory, "master.m3u8")?;
        return serve_file_if_exists(static_path, headers).await;
    }

    let (item_id, media_type) = media_type_item_id(uri)?;
    let job_id = existing_or_computed_job_id(query, item_id, media_type);
    let main_url = format!(
        "main.m3u8?{}&jobId={}",
        uri.query().unwrap_or_default(),
        job_id
    );
    start_hls_job(state, query, item_id, &job_id, identity, media_type).await?;
    let path =
        resolve_transcode_file(&state.transcode_directory, &format!("{job_id}.master.m3u8"))?;
    let content = build_variant_master_playlist(&master_variants(query, &main_url));
    tokio::fs::write(&path, content)
        .await
        .map_err(|_| ApiError::Internal)?;
    serve_file(path, headers).await
}

fn master_variants(query: &TranscodeQuery, main_url: &str) -> Vec<HlsVariant> {
    let (width, height) = match (query.max_width, query.max_height) {
        (Some(width), Some(height)) => (width, height),
        (Some(width), None) => (width, 1_080),
        (None, Some(height)) => (1_920, height),
        (None, None) => {
            return vec![
                HlsVariant::new(2_000_000, "640x360", main_url),
                HlsVariant::new(5_000_000, "1280x720", main_url),
                HlsVariant::new(8_000_000, "1920x1080", main_url),
            ];
        }
    };
    #[allow(clippy::cast_sign_loss)]
    let bandwidth = query
        .video_bitrate
        .map_or(8_000_000, |bitrate| bitrate.max(1)) as u64;
    vec![HlsVariant::new(
        bandwidth,
        format!("{width}x{height}"),
        main_url,
    )]
}

async fn ensure_main_playlist(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    query: &TranscodeQuery,
    identity: &crate::authentication::AuthenticatedIdentity,
) -> Result<Response, ApiError> {
    if query.job_id.is_none() && !query.has_transcode_parameters() {
        let static_path = resolve_transcode_file(&state.transcode_directory, "main.m3u8")?;
        return serve_file_if_exists(static_path, headers).await;
    }

    let (item_id, media_type) = media_type_item_id(uri)?;
    let job_id = existing_or_computed_job_id(query, item_id, media_type);
    start_hls_job(state, query, item_id, &job_id, identity, media_type).await?;
    let path = resolve_transcode_file(&state.transcode_directory, &format!("{job_id}.m3u8"))?;
    serve_file_if_exists(path, headers).await
}

async fn start_hls_job(
    state: &AppState,
    query: &TranscodeQuery,
    item_id: Uuid,
    job_id: &str,
    identity: &crate::authentication::AuthenticatedIdentity,
    media_type: &str,
) -> Result<(), ApiError> {
    if state.transcode_jobs.is_running(job_id) {
        if let (Some(device_id), Some(play_session_id)) =
            (query.device_id.as_deref(), query.play_session_id.as_deref())
        {
            state
                .transcode_jobs
                .associate(job_id, device_id, play_session_id);
        }
        return Ok(());
    }
    let user = match identity {
        crate::authentication::AuthenticatedIdentity::Device(session) => &session.user,
        crate::authentication::AuthenticatedIdentity::ApiKey(_) => {
            return Err(ApiError::NotFound);
        }
    };
    let target = TranscodeTarget {
        is_video: media_type == "Videos",
        hwaccel: query.hwaccel.clone(),
        video_codec: query
            .video_codec
            .clone()
            .or_else(|| Some("h264".to_owned())),
        audio_codec: query.audio_codec.clone().or_else(|| Some("aac".to_owned())),
        video_bitrate: query.video_bitrate,
        audio_bitrate: query.audio_bitrate,
        audio_channels: query.max_audio_channels,
        audio_sample_rate: query.audio_sample_rate,
        audio_stream_index: query.audio_stream_index,
        subtitle_index: query.subtitle_stream_index,
        burn_subtitles: query.burn_subtitles.unwrap_or(false),
        audio_normalize: query.audio_normalize.unwrap_or(false),
        tonemap_hdr: query.enable_hdr_tone_mapping.unwrap_or(false),
        max_width: query.max_width,
        max_height: query.max_height,
        max_framerate: query.max_framerate,
        start_time_ticks: query.start_time_ticks,
    };
    let settings = HlsSegmentSettings {
        container: query
            .segment_container
            .clone()
            .unwrap_or_else(|| "ts".to_owned()),
        segment_length_ms: query.segment_length.unwrap_or(6_000),
        min_segments: query.min_segments.unwrap_or(2),
    };
    let item = state
        .library_controller
        .item(user, user.id, item_id)
        .await?;
    let input = item.path.ok_or(ApiError::NotFound)?;
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let output_prefix = state.transcode_directory.join(job_id);
    let main = build_main_playlist(item_id, job_id, item.runtime_ticks, &settings, media_type)
        .map_err(|_| ApiError::Internal)?;
    tokio::fs::write(
        state.transcode_directory.join(format!("{job_id}.m3u8")),
        main,
    )
    .await
    .map_err(|_| ApiError::Internal)?;
    let command = hls_command(
        &state.ffmpeg_path,
        std::path::Path::new(&input),
        &output_prefix,
        &target,
        &settings,
    );
    let job = match (query.device_id.as_deref(), query.play_session_id.as_deref()) {
        (Some(device_id), Some(play_session_id)) => state
            .transcode_jobs
            .register_for_session_with_path(job_id, device_id, play_session_id, &input),
        _ => state.transcode_jobs.register(job_id),
    };
    let jobs = state.transcode_jobs.clone();
    let finished_job_id = job_id.to_owned();
    tokio::spawn(async move {
        if let Err(error) = run_ffmpeg(&command, &job).await {
            eprintln!("HLS transcode failed: {error}");
        }
        jobs.remove(&finished_job_id);
    });
    wait_for_segment(
        &state.transcode_directory,
        job_id,
        settings.container.trim_start_matches('.'),
    )
    .await
    .map_err(|_| ApiError::Internal)
}

pub(crate) async fn cleanup_transcode_job(root: &FilePath, job_id: &str) {
    if job_id.is_empty() {
        return;
    }
    let Ok(mut entries) = fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_job_file = name.strip_prefix(job_id).is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with('.')
                || rest
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_digit())
        });
        if is_job_file {
            let _ = fs::remove_file(entry.path()).await;
        }
    }
}

fn media_type_item_id(uri: &Uri) -> Result<(Uuid, &'static str), ApiError> {
    let segments = uri.path().split('/').collect::<Vec<_>>();
    let (media_type, item_index) = segments
        .iter()
        .position(|segment| *segment == "Videos" || *segment == "Audio")
        .map(|index| {
            let media_type = if segments[index] == "Videos" {
                "Videos"
            } else {
                "Audio"
            };
            (media_type, index + 1)
        })
        .ok_or(ApiError::InvalidRequest)?;
    let item_id = segments
        .get(item_index)
        .ok_or(ApiError::InvalidRequest)?
        .parse::<Uuid>()
        .map_err(|_| ApiError::InvalidRequest)?;
    Ok((item_id, media_type))
}

fn existing_or_computed_job_id(query: &TranscodeQuery, item_id: Uuid, media_type: &str) -> String {
    query
        .job_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map_or_else(|| compute_job_id(item_id, query, media_type), str::to_owned)
}

fn compute_job_id(item_id: Uuid, query: &TranscodeQuery, media_type: &str) -> String {
    let target = TranscodeTarget {
        is_video: media_type == "Videos",
        hwaccel: query.hwaccel.clone(),
        video_codec: query.video_codec.clone(),
        audio_codec: query.audio_codec.clone(),
        video_bitrate: query.video_bitrate,
        audio_bitrate: query.audio_bitrate,
        audio_channels: query.max_audio_channels,
        audio_sample_rate: None,
        audio_stream_index: query.audio_stream_index,
        subtitle_index: query.subtitle_stream_index,
        burn_subtitles: query.burn_subtitles.unwrap_or(false),
        audio_normalize: query.audio_normalize.unwrap_or(false),
        tonemap_hdr: query.enable_hdr_tone_mapping.unwrap_or(false),
        max_width: query.max_width,
        max_height: query.max_height,
        max_framerate: query.max_framerate,
        start_time_ticks: query.start_time_ticks,
    };
    let settings = HlsSegmentSettings {
        container: query
            .segment_container
            .clone()
            .unwrap_or_else(|| "ts".to_owned()),
        segment_length_ms: query.segment_length.unwrap_or(6_000),
        min_segments: query.min_segments.unwrap_or(2),
    };
    hls_job_id(
        item_id,
        query.media_source_id.as_deref(),
        query.start_time_ticks,
        &target,
        &settings,
    )
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
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

async fn serve_file_if_exists(
    path: std::path::PathBuf,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    if !tokio::fs::try_exists(&path)
        .await
        .map_err(|_| ApiError::Internal)?
    {
        return Ok((StatusCode::NOT_FOUND, "Hls playlist not found.").into_response());
    }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use uuid::Uuid;

    use super::cleanup_transcode_job;

    #[tokio::test]
    async fn cleanup_transcode_job_removes_only_job_prefixed_files() {
        let root =
            std::env::temp_dir().join(format!("jellyfin-hls-cleanup-{}", Uuid::new_v4().simple()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let job_id = "abc12345-job";
        for name in [
            format!("{job_id}.m3u8"),
            format!("{job_id}0.ts"),
            format!("{job_id}.master.m3u8"),
        ] {
            tokio::fs::write(root.join(name), b"segment").await.unwrap();
        }
        let other = root.join("other.m3u8");
        let prefix_cousin = root.join(format!("{job_id}-other.ts"));
        tokio::fs::write(&other, b"other").await.unwrap();
        tokio::fs::write(&prefix_cousin, b"cousin").await.unwrap();

        cleanup_transcode_job(&root, job_id).await;

        assert!(!root.join(format!("{job_id}.m3u8")).exists());
        assert!(!root.join(format!("{job_id}0.ts")).exists());
        assert!(!root.join(format!("{job_id}.master.m3u8")).exists());
        assert!(other.exists());
        assert!(prefix_cousin.exists());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_transcode_job_tolerates_missing_directory() {
        let root: PathBuf = std::env::temp_dir().join(format!(
            "jellyfin-hls-cleanup-missing-{}",
            Uuid::new_v4().simple()
        ));
        cleanup_transcode_job(&root, "job").await;
    }
}
