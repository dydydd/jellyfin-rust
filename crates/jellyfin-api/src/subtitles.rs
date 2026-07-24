use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path as AxumPath, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Utc};
use jellyfin_data::NamedConfigurationStoreError;
use jellyfin_model::{FontFile, MediaStream, MediaStreamType, MimeTypes, RemoteSubtitleInfo};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization};

const SUPPORTED_FONT_EXTENSIONS: &[&str] = &["woff", "woff2", "ttf", "otf"];
const MAX_FONT_LIST_BYTES: i64 = 20_971_520;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct EncodingOptionsSubset {
    fallback_font_path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct RemoteSubtitleSearchQuery {
    #[serde(alias = "isPerfectMatch")]
    is_perfect_match: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct UploadSubtitleDto {
    language: Option<String>,
    format: Option<String>,
    is_forced: Option<bool>,
    is_hearing_impaired: Option<bool>,
    data: Option<String>,
}

pub(crate) async fn delete_subtitle(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    AxumPath((item_id, index)): AxumPath<(Uuid, i32)>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;
    state
        .media_streams
        .delete_media_stream(item_id, index, MediaStreamType::Subtitle)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn upload_subtitle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(item_id): AxumPath<Uuid>,
    request: Result<Json<UploadSubtitleDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_subtitles() {
        return Err(ApiError::Forbidden);
    }
    ensure_video_item(&state, &authenticated.user, item_id).await?;

    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let language = subtitle_token(request.language.as_deref()).ok_or(ApiError::InvalidRequest)?;
    let format = subtitle_token(request.format.as_deref()).ok_or(ApiError::InvalidRequest)?;
    let is_forced = request.is_forced.ok_or(ApiError::InvalidRequest)?;
    let is_hearing_impaired = request
        .is_hearing_impaired
        .ok_or(ApiError::InvalidRequest)?;
    let encoded = request.data.ok_or(ApiError::InvalidRequest)?;
    let subtitle = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::InvalidRequest)?;
    if subtitle.is_empty() {
        return Err(ApiError::InvalidRequest);
    }

    let mut streams = state
        .media_streams
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item_id))
        .await?;
    let index = streams
        .iter()
        .map(|stream| stream.index)
        .max()
        .unwrap_or(-1)
        + 1;
    let path = uploaded_subtitle_path(&state, item_id, index, &language, &format);
    if let Some(directory) = path.parent() {
        tokio::fs::create_dir_all(directory)
            .await
            .map_err(|_| ApiError::Internal)?;
    }
    let temporary_path = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    tokio::fs::write(&temporary_path, subtitle)
        .await
        .map_err(|_| ApiError::Internal)?;
    tokio::fs::rename(&temporary_path, &path)
        .await
        .map_err(|_| ApiError::Internal)?;

    streams.push(MediaStream {
        index,
        stream_type: MediaStreamType::Subtitle,
        codec: Some(format),
        language: Some(language),
        is_external: true,
        is_forced,
        is_hearing_impaired,
        path: Some(path.to_string_lossy().into_owned()),
        ..MediaStream::default()
    });
    if let Err(error) = state
        .media_streams
        .save_media_streams(item_id, &streams)
        .await
    {
        let _ = tokio::fs::remove_file(path).await;
        return Err(error.into());
    }

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn search_remote_subtitles(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((item_id, language)): AxumPath<(Uuid, String)>,
    Query(query): Query<RemoteSubtitleSearchQuery>,
) -> Result<Json<Vec<RemoteSubtitleInfo>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_subtitles() {
        return Err(ApiError::Forbidden);
    }
    ensure_video_item(&state, &authenticated.user, item_id).await?;

    // Remote subtitle providers are not wired yet. The query is parsed so the
    // route keeps Jellyfin's official parameter surface while returning the
    // empty provider aggregate clients expect on an installation without
    // subtitle providers.
    let RemoteSubtitleSearchQuery {
        is_perfect_match: _is_perfect_match,
    } = query;
    let _language = language;
    Ok(Json(Vec::new()))
}

pub(crate) async fn download_remote_subtitles(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath((item_id, subtitle_id)): AxumPath<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_subtitles() {
        return Err(ApiError::Forbidden);
    }
    ensure_video_item(&state, &authenticated.user, item_id).await?;

    // Jellyfin logs provider download failures and still returns NoContent.
    // With no providers registered there is nothing to persist or refresh yet.
    let _subtitle_id = subtitle_id;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn get_remote_subtitles(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(_subtitle_id): AxumPath<String>,
) -> Result<Response, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    if !authenticated.can_manage_subtitles() {
        return Err(ApiError::Forbidden);
    }

    Ok(StatusCode::NOT_FOUND.into_response())
}

pub(crate) async fn fallback_fonts(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FontFile>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Some(directory) = fallback_font_directory(&state).await? else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(list_font_files(&directory)?))
}

pub(crate) async fn fallback_font(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    AxumPath(name): AxumPath<String>,
) -> Result<Response, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Some(directory) = fallback_font_directory(&state).await? else {
        return Ok(StatusCode::OK.into_response());
    };
    let Some(path) = find_font_file(&directory, &name)? else {
        return Ok(StatusCode::OK.into_response());
    };
    if fs::metadata(&path).map_err(|_| ApiError::Internal)?.len() == 0 {
        return Ok(StatusCode::OK.into_response());
    }

    let request = Request::builder()
        .method("GET")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    let response = match ServeFile::new(&path).oneshot(request).await {
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

async fn fallback_font_directory(state: &AppState) -> Result<Option<PathBuf>, ApiError> {
    let Some(repository) = state.named_configurations.as_ref() else {
        return Ok(None);
    };
    let configuration = match repository.load("encoding").await {
        Ok(configuration) => configuration.configuration,
        Err(NamedConfigurationStoreError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let options: EncodingOptionsSubset =
        serde_json::from_value(configuration).map_err(|_| ApiError::Internal)?;
    let path = options.fallback_font_path.trim();
    if path.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(path)))
    }
}

fn list_font_files(directory: &Path) -> Result<Vec<FontFile>, ApiError> {
    let mut fonts = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ApiError::Internal),
    };
    for entry in entries {
        let entry = entry.map_err(|_| ApiError::Internal)?;
        let path = entry.path();
        if !path.is_file() || !is_supported_font_path(&path) {
            continue;
        }
        let metadata = entry.metadata().map_err(|_| ApiError::Internal)?;
        fonts.push(FontFile {
            name: entry.file_name().to_str().map(ToOwned::to_owned),
            size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
            date_created: metadata_time(metadata.created().ok()),
            date_modified: metadata_time(metadata.modified().ok()),
        });
    }
    fonts.sort_by(|left, right| {
        left.size
            .cmp(&right.size)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| right.date_modified.cmp(&left.date_modified))
            .then_with(|| right.date_created.cmp(&left.date_created))
    });

    let mut total = 0_i64;
    let mut limited = Vec::new();
    for font in fonts {
        total = total.saturating_add(font.size);
        if total >= MAX_FONT_LIST_BYTES {
            break;
        }
        limited.push(font);
    }
    Ok(limited)
}

fn find_font_file(directory: &Path, requested_name: &str) -> Result<Option<PathBuf>, ApiError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ApiError::Internal),
    };
    for entry in entries {
        let entry = entry.map_err(|_| ApiError::Internal)?;
        let path = entry.path();
        if !path.is_file() || !is_supported_font_path(&path) {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(requested_name))
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn is_supported_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_FONT_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn metadata_time(time: Option<SystemTime>) -> DateTime<Utc> {
    time.map(DateTime::<Utc>::from)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

async fn ensure_video_item(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    item_id: Uuid,
) -> Result<jellyfin_data::entities::base_item::Model, ApiError> {
    let item = state
        .user_library
        .item(authenticated_user, authenticated_user.id, item_id)
        .await?;
    if item.media_type.as_deref().is_some_and(|media_type| {
        media_type.eq_ignore_ascii_case("Video")
            || media_type.eq_ignore_ascii_case("VideoFile")
            || media_type.eq_ignore_ascii_case("VideoStream")
    }) || matches!(
        item.item_type.as_str(),
        "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
    ) {
        Ok(item)
    } else {
        Err(jellyfin_controller::UserLibraryError::ItemNotFound.into())
    }
}

fn subtitle_token(value: Option<&str>) -> Option<String> {
    let value = value?.trim().trim_start_matches('.');
    (!value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then(|| value.to_ascii_lowercase())
}

fn uploaded_subtitle_path(
    state: &AppState,
    item_id: Uuid,
    index: i32,
    language: &str,
    format: &str,
) -> PathBuf {
    state
        .internal_metadata_directory
        .join("subtitles")
        .join(item_id.simple().to_string())
        .join(format!("{index}.{language}.{format}"))
}
