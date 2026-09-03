use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
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
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{ApiError, AppState, authorization};

const MANIFEST_ENTRY_NAME: &str = "manifest.json";
const BACKUP_ENGINE_VERSION: &str = "1.0";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
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

#[derive(Debug)]
enum ArchiveValidationError {
    Invalid(String),
    Unsupported(String),
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
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let options = request.map_or_else(|_| BackupOptionsDto::default(), |Json(options)| options);
    if options.database {
        return Ok(status_message(
            StatusCode::NOT_IMPLEMENTED,
            "PostgreSQL backup is not implemented safely; retry with Database=false",
        ));
    }

    let backup_directory = state.program_data_directory.join("backups");
    tokio::fs::create_dir_all(&backup_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let date_created = Utc::now();
    let unique_id = Uuid::new_v4().simple();
    let file_name = format!(
        "jellyfin-backup-{}-{unique_id}.zip",
        date_created.format("%Y%m%d%H%M%S")
    );
    let archive_path = backup_directory.join(file_name);
    let temporary_path = backup_directory.join(format!(".{unique_id}.partial"));
    let manifest = BackupManifestDto {
        server_version: state
            .system_info
            .version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned()),
        backup_engine_version: BACKUP_ENGINE_VERSION.to_owned(),
        date_created,
        path: archive_path.to_string_lossy().into_owned(),
        options,
    };
    let program_data_directory = state.program_data_directory.clone();
    let internal_metadata_directory = state.internal_metadata_directory.clone();
    let result = tokio::task::spawn_blocking(move || {
        let result = create_backup_archive_blocking(
            &temporary_path,
            &manifest,
            &program_data_directory,
            &internal_metadata_directory,
        )
        .and_then(|()| fs::rename(&temporary_path, &archive_path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result.map(|()| manifest)
    })
    .await
    .map_err(|_| ApiError::Internal)?
    .map_err(|_| ApiError::Internal)?;
    Ok(Json(result).into_response())
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

    let validation = tokio::task::spawn_blocking(move || validate_archive_blocking(&archive_path))
        .await
        .map_err(|_| ApiError::Internal)?;
    match validation {
        Ok(_) => Ok(status_message(
            StatusCode::NOT_IMPLEMENTED,
            "Validated backup, but online restore is not supported safely; no data was changed",
        )),
        Err(ArchiveValidationError::Invalid(message)) => {
            Ok(status_message(StatusCode::BAD_REQUEST, message))
        }
        Err(ArchiveValidationError::Unsupported(message)) => {
            Ok(status_message(StatusCode::UNPROCESSABLE_ENTITY, message))
        }
    }
}

async fn load_manifest(path: PathBuf) -> Result<Option<BackupManifestDto>, ApiError> {
    tokio::task::spawn_blocking(move || load_manifest_blocking(&path))
        .await
        .map_err(|_| ApiError::Internal)
}

fn load_manifest_blocking(path: &Path) -> Option<BackupManifestDto> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut manifest_entry = archive.by_name(MANIFEST_ENTRY_NAME).ok()?;
    if manifest_entry.size() > MAX_MANIFEST_BYTES {
        return None;
    }
    let mut manifest_json = String::new();
    manifest_entry.read_to_string(&mut manifest_json).ok()?;
    let manifest = serde_json::from_str::<BackupManifest>(&manifest_json).ok()?;
    Some(manifest_dto(path, manifest))
}

fn create_backup_archive_blocking(
    path: &Path,
    manifest: &BackupManifestDto,
    program_data_directory: &Path,
    internal_metadata_directory: &Path,
) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let directory_options = SimpleFileOptions::default().unix_permissions(0o700);

    archive.add_directory("Data/", directory_options)?;
    let excluded = [
        program_data_directory.join("backups"),
        program_data_directory.join("metadata"),
        program_data_directory.join("trickplay"),
        program_data_directory.join("subtitles"),
    ];
    add_directory_tree(
        &mut archive,
        program_data_directory,
        "Data",
        &excluded,
        options,
        directory_options,
    )?;

    if manifest.options.metadata {
        archive.add_directory("Data/metadata/", directory_options)?;
        add_directory_tree(
            &mut archive,
            internal_metadata_directory,
            "Data/metadata",
            &[],
            options,
            directory_options,
        )?;
    }
    if manifest.options.trickplay {
        archive.add_directory("Data/trickplay/", directory_options)?;
        add_directory_tree(
            &mut archive,
            &program_data_directory.join("trickplay"),
            "Data/trickplay",
            &[],
            options,
            directory_options,
        )?;
    }
    if manifest.options.subtitles {
        archive.add_directory("Data/subtitles/", directory_options)?;
        add_directory_tree(
            &mut archive,
            &program_data_directory.join("subtitles"),
            "Data/subtitles",
            &[],
            options,
            directory_options,
        )?;
    }

    archive.start_file(MANIFEST_ENTRY_NAME, options)?;
    archive.write_all(&serde_json::to_vec(manifest).map_err(std::io::Error::other)?)?;
    archive.finish()?;
    Ok(())
}

fn add_directory_tree(
    archive: &mut ZipWriter<File>,
    source: &Path,
    archive_root: &str,
    excluded: &[PathBuf],
    file_options: SimpleFileOptions,
    directory_options: SimpleFileOptions,
) -> std::io::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(format!(
            "refusing to back up symbolic link {}",
            source.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::other(format!(
            "backup source is not a directory: {}",
            source.display()
        )));
    }

    let mut pending = vec![source.to_owned()];
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let item_path = entry.path();
            if excluded.iter().any(|excluded_path| {
                item_path == *excluded_path || item_path.starts_with(excluded_path)
            }) {
                continue;
            }
            let item_metadata = fs::symlink_metadata(&item_path)?;
            if item_metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(format!(
                    "refusing to back up symbolic link {}",
                    item_path.display()
                )));
            }
            let relative = item_path
                .strip_prefix(source)
                .map_err(std::io::Error::other)?;
            let entry_name = archive_entry_name(archive_root, relative)?;
            if item_metadata.is_dir() {
                archive.add_directory(format!("{entry_name}/"), directory_options)?;
                pending.push(item_path);
            } else if item_metadata.is_file() {
                archive.start_file(entry_name, file_options)?;
                let mut input = File::open(item_path)?;
                std::io::copy(&mut input, archive)?;
            } else {
                return Err(std::io::Error::other("unsupported backup source file type"));
            }
        }
    }
    Ok(())
}

fn archive_entry_name(root: &str, relative: &Path) -> std::io::Result<String> {
    let mut name = root.to_owned();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(std::io::Error::other("invalid backup source path"));
        };
        let component = component
            .to_str()
            .ok_or_else(|| std::io::Error::other("backup paths must be valid UTF-8"))?;
        name.push('/');
        name.push_str(component);
    }
    Ok(name)
}

fn validate_archive_blocking(path: &Path) -> Result<BackupManifestDto, ArchiveValidationError> {
    let file = File::open(path)
        .map_err(|error| ArchiveValidationError::Invalid(format!("cannot read backup: {error}")))?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        ArchiveValidationError::Invalid(format!("invalid ZIP archive: {error}"))
    })?;
    let mut names = HashSet::new();
    let mut manifest_json = None;
    let mut total_size = 0_u64;
    let mut has_data_root = false;
    let mut has_database_payload = false;
    let mut has_metadata_root = false;
    let mut has_trickplay_root = false;
    let mut has_subtitles_root = false;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            ArchiveValidationError::Invalid(format!("cannot read ZIP entry: {error}"))
        })?;
        let raw_name = entry.name().to_owned();
        if raw_name.contains('\\') || entry.enclosed_name().is_none() {
            return Err(ArchiveValidationError::Invalid(format!(
                "unsafe ZIP entry path: {raw_name}"
            )));
        }
        if !names.insert(raw_name.clone()) {
            return Err(ArchiveValidationError::Invalid(format!(
                "duplicate ZIP entry: {raw_name}"
            )));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(ArchiveValidationError::Invalid(format!(
                "symbolic links are not allowed in backups: {raw_name}"
            )));
        }
        total_size = total_size
            .checked_add(entry.size())
            .ok_or_else(|| ArchiveValidationError::Invalid("archive size overflow".to_owned()))?;
        if total_size > MAX_ARCHIVE_UNCOMPRESSED_BYTES {
            return Err(ArchiveValidationError::Invalid(
                "archive expands beyond the safety limit".to_owned(),
            ));
        }

        let is_allowed = raw_name == MANIFEST_ENTRY_NAME
            || raw_name.starts_with("Config/")
            || raw_name.starts_with("Data/")
            || raw_name.starts_with("Root/")
            || raw_name.starts_with("Database/");
        if !is_allowed {
            return Err(ArchiveValidationError::Invalid(format!(
                "unexpected ZIP entry: {raw_name}"
            )));
        }
        has_data_root |= raw_name == "Data/" || raw_name.starts_with("Data/");
        has_database_payload |= raw_name.starts_with("Database/") && !entry.is_dir();
        has_metadata_root |= raw_name == "Data/metadata/" || raw_name.starts_with("Data/metadata/");
        has_trickplay_root |=
            raw_name == "Data/trickplay/" || raw_name.starts_with("Data/trickplay/");
        has_subtitles_root |=
            raw_name == "Data/subtitles/" || raw_name.starts_with("Data/subtitles/");

        if raw_name == MANIFEST_ENTRY_NAME {
            if entry.is_dir() || entry.size() > MAX_MANIFEST_BYTES {
                return Err(ArchiveValidationError::Invalid(
                    "manifest.json is not a small regular file".to_owned(),
                ));
            }
            let mut json = String::new();
            entry.read_to_string(&mut json).map_err(|error| {
                ArchiveValidationError::Invalid(format!("cannot read manifest.json: {error}"))
            })?;
            manifest_json = Some(json);
        }
    }

    let manifest_json = manifest_json.ok_or_else(|| {
        ArchiveValidationError::Invalid("backup is missing manifest.json".to_owned())
    })?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_json).map_err(|error| {
        ArchiveValidationError::Invalid(format!("invalid manifest.json: {error}"))
    })?;
    if manifest.server_version.trim().is_empty() {
        return Err(ArchiveValidationError::Invalid(
            "manifest ServerVersion is empty".to_owned(),
        ));
    }
    if manifest.backup_engine_version != BACKUP_ENGINE_VERSION {
        return Err(ArchiveValidationError::Unsupported(format!(
            "unsupported backup engine version {}; expected {BACKUP_ENGINE_VERSION}",
            manifest.backup_engine_version
        )));
    }
    if !has_data_root {
        return Err(ArchiveValidationError::Invalid(
            "backup contains no Data entries".to_owned(),
        ));
    }
    if manifest.options.database && !has_database_payload {
        return Err(ArchiveValidationError::Invalid(
            "manifest requests database restore but the archive has no Database payload".to_owned(),
        ));
    }
    if !manifest.options.database && has_database_payload {
        return Err(ArchiveValidationError::Invalid(
            "archive contains Database payload although the manifest disables it".to_owned(),
        ));
    }
    for (enabled, present, name) in [
        (manifest.options.metadata, has_metadata_root, "metadata"),
        (manifest.options.trickplay, has_trickplay_root, "trickplay"),
        (manifest.options.subtitles, has_subtitles_root, "subtitles"),
    ] {
        if enabled && !present {
            return Err(ArchiveValidationError::Invalid(format!(
                "manifest requests {name} restore but its Data/{name} payload is missing"
            )));
        }
        if !enabled && present {
            return Err(ArchiveValidationError::Invalid(format!(
                "archive contains Data/{name} although the manifest disables it"
            )));
        }
    }
    Ok(manifest_dto(path, manifest))
}

fn manifest_dto(path: &Path, manifest: BackupManifest) -> BackupManifestDto {
    BackupManifestDto {
        server_version: manifest.server_version,
        backup_engine_version: manifest.backup_engine_version,
        date_created: manifest.date_created,
        path: path.to_string_lossy().into_owned(),
        options: manifest.options,
    }
}

fn sanitized_backup_path(state: &AppState, path: &str) -> Option<PathBuf> {
    let requested = Path::new(path);
    let backup_directory = state.program_data_directory.join("backups");
    let candidate = if requested.is_absolute() {
        requested.to_owned()
    } else {
        if requested.components().count() != 1 {
            return None;
        }
        backup_directory.join(requested)
    };
    let file_name = candidate.file_name()?.to_str()?;
    if file_name.trim().is_empty()
        || !candidate
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        return None;
    }
    // The manifest exposes the archive's full path. Accept an existing file
    // only when it canonically resides directly in the configured backup
    // directory; this keeps path traversal and symlink escapes out.
    if fs::symlink_metadata(&candidate)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return None;
    }
    let canonical_backup = fs::canonicalize(&backup_directory).ok()?;
    let canonical_candidate = fs::canonicalize(&candidate).ok()?;
    (canonical_candidate.parent() == Some(canonical_backup.as_path())).then_some(candidate)
}

fn status_message(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}
