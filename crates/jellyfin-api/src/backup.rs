use std::{fs::File, io::Read, path::PathBuf, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use jellyfin_model::{BackupManifestDto, BackupOptionsDto};
use serde::Deserialize;
use zip::ZipArchive;

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
