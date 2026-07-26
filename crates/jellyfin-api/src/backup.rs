use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json,
    extract::rejection::JsonRejection,
    extract::{OriginalUri, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use jellyfin_model::{BackupManifestDto, BackupOptionsDto, BackupRestoreRequestDto};
use serde::Deserialize;
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{ApiError, AppState, authorization};

const MANIFEST_ENTRY_NAME: &str = "manifest.json";

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct BackupManifest {
    server_version: String,
    backup_engine_version: String,
    date_created: DateTime<Utc>,
    options: BackupOptionsDto,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BackupManifestQuery {
    path: String,
}

impl Default for BackupManifest {
    fn default() -> Self {
        Self {
            server_version: String::new(),
            backup_engine_version: String::new(),
            date_created: DateTime::<Utc>::UNIX_EPOCH,
            options: BackupOptionsDto::default(),
        }
    }
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<BackupManifestDto>>, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let backup_directory = state.program_data_directory.join("backups");
    let mut entries = match tokio::fs::read_dir(&backup_directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Json(Vec::new()));
        }
        Err(_) => return Err(ApiError::Internal),
    };
    let mut archives = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|_| ApiError::Internal)? {
        let path = entry.path();
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            archives.push(path);
        }
    }
    archives.sort();

    let mut manifests = Vec::new();
    for archive in archives {
        if let Some(manifest) = load_manifest(archive).await? {
            manifests.push(manifest);
        }
    }
    Ok(Json(manifests))
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<BackupOptionsDto>, JsonRejection>,
) -> Result<Json<BackupManifestDto>, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let options = request.map_or_else(|_| BackupOptionsDto::default(), |Json(options)| options);
    let backup_directory = state.program_data_directory.join("backups");
    tokio::fs::create_dir_all(&backup_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let date_created = Utc::now();
    let file_name = format!(
        "jellyfin-backup-{}-{}.zip",
        date_created.format("%Y%m%d%H%M%S"),
        Uuid::new_v4().simple()
    );
    let archive_path = backup_directory.join(file_name);
    let manifest = BackupManifestDto {
        server_version: state
            .system_info
            .version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned()),
        backup_engine_version: "1.0".to_owned(),
        date_created,
        path: archive_path.to_string_lossy().into_owned(),
        options,
    };
    let manifest_for_archive = manifest.clone();
    tokio::task::spawn_blocking(move || {
        create_backup_archive_blocking(&archive_path, &manifest_for_archive)
    })
    .await
    .map_err(|_| ApiError::Internal)?
    .map_err(|_| ApiError::Internal)?;
    Ok(Json(manifest))
}

pub(crate) async fn manifest(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<BackupManifestQuery>,
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let Some(archive_path) = sanitized_backup_path(&state, &query.path) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if !archive_path.is_file() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let Some(manifest) = load_manifest(archive_path).await? else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    Ok(Json(manifest).into_response())
}

pub(crate) async fn restore(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<BackupRestoreRequestDto>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let Some(archive_path) = sanitized_backup_path(&state, &request.archive_file_name) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if !archive_path.is_file() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn load_manifest(path: PathBuf) -> Result<Option<BackupManifestDto>, ApiError> {
    tokio::task::spawn_blocking(move || load_manifest_blocking(path))
        .await
        .map_err(|_| ApiError::Internal)
}

fn load_manifest_blocking(path: PathBuf) -> Option<BackupManifestDto> {
    let file = File::open(&path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut manifest_entry = archive.by_name(MANIFEST_ENTRY_NAME).ok()?;
    let mut manifest_json = String::new();
    manifest_entry.read_to_string(&mut manifest_json).ok()?;
    let manifest = serde_json::from_str::<BackupManifest>(&manifest_json).ok()?;
    Some(BackupManifestDto {
        server_version: manifest.server_version,
        backup_engine_version: manifest.backup_engine_version,
        date_created: manifest.date_created,
        path: path.to_string_lossy().into_owned(),
        options: manifest.options,
    })
}

fn create_backup_archive_blocking(
    path: &Path,
    manifest: &BackupManifestDto,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = File::create(path)?;
    let mut archive = ZipWriter::new(file);
    archive.start_file(MANIFEST_ENTRY_NAME, SimpleFileOptions::default())?;
    let manifest_json = serde_json::to_vec(manifest)?;
    archive.write_all(&manifest_json)?;
    archive.finish()?;
    Ok(())
}

fn sanitized_backup_path(state: &AppState, path: &str) -> Option<PathBuf> {
    let file_name = Path::new(path).file_name()?.to_str()?;
    if file_name.trim().is_empty() {
        return None;
    }
    Some(state.program_data_directory.join("backups").join(file_name))
}
