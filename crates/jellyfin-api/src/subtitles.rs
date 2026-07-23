use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use jellyfin_data::NamedConfigurationStoreError;
use jellyfin_model::{FontFile, MimeTypes};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::{ApiError, AppState, authentication};

const SUPPORTED_FONT_EXTENSIONS: &[&str] = &["woff", "woff2", "ttf", "otf"];
const MAX_FONT_LIST_BYTES: i64 = 20_971_520;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct EncodingOptionsSubset {
    fallback_font_path: String,
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
