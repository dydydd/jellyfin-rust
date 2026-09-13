use std::{
    collections::HashSet,
    fmt,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json,
    body::Bytes,
    extract::rejection::JsonRejection,
    extract::{OriginalUri, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use jellyfin_model::{BackupManifestDto, BackupOptionsDto, BackupRestoreRequestDto};
use sea_orm::{
    AccessMode, ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend,
    IsolationLevel, Statement, StreamTrait, TransactionTrait,
};
use serde::{Deserialize, Deserializer, Serialize, de, de::DeserializeSeed};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::{ApiError, AppState, authorization};

const MANIFEST_ENTRY_NAME: &str = "manifest.json";
const BACKUP_ENGINE_VERSION: &str = "1.0";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const DATABASE_RESTORE_BATCH_SIZE: usize = 128;
const BACKUP_DATABASE_LOCK_KEY: i64 = 0x4a46_4241_434b_5550;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct BackupManifest {
    server_version: String,
    backup_engine_version: String,
    date_created: DateTime<Utc>,
    #[serde(default)]
    database_tables: Vec<String>,
    options: BackupOptionsDto,
}

#[derive(Debug)]
struct DatabaseBackupTable {
    schema: String,
    name: String,
}

#[derive(Debug)]
struct CaseInsensitiveBackupOptions(BackupOptionsDto);

impl<'de> Deserialize<'de> for CaseInsensitiveBackupOptions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OptionsVisitor;

        impl<'de> de::Visitor<'de> for OptionsVisitor {
            type Value = CaseInsensitiveBackupOptions;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a backup options object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut options = BackupOptionsDto::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("Metadata") {
                        options.metadata = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("Trickplay") {
                        options.trickplay = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("Subtitles") {
                        options.subtitles = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("Database") {
                        options.database = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(CaseInsensitiveBackupOptions(options))
            }
        }

        deserializer.deserialize_map(OptionsVisitor)
    }
}

#[derive(Debug)]
pub(crate) struct CaseInsensitiveBackupRestoreRequest(BackupRestoreRequestDto);

impl<'de> Deserialize<'de> for CaseInsensitiveBackupRestoreRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RestoreVisitor;

        impl<'de> de::Visitor<'de> for RestoreVisitor {
            type Value = CaseInsensitiveBackupRestoreRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a backup restore request object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut archive_file_name = String::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("ArchiveFileName") {
                        archive_file_name = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(CaseInsensitiveBackupRestoreRequest(
                    BackupRestoreRequestDto { archive_file_name },
                ))
            }
        }

        deserializer.deserialize_map(RestoreVisitor)
    }
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

/// Backup manifests for protocol adapters that need the shared authorization
/// and archive discovery implementation.
pub async fn emby_list(
    state: State<Arc<AppState>>,
    uri: OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<BackupManifestDto>>, Response> {
    list(state, uri, headers)
        .await
        .map_err(IntoResponse::into_response)
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let options = parse_create_options(&body)?;
    // The official backup service rejects snapshots during library scans: a
    // scan updates PostgreSQL and its associated files independently, so a
    // database snapshot plus a concurrent file walk could not be coherent.
    if state.library_scan.is_scan_running() {
        return Ok(status_message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Cannot create a backup while a library scan is running. Please try again once the scan has finished.",
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
    let database_staging_path = backup_directory.join(format!(".{unique_id}.database"));
    let database_tables = if options.database {
        match export_database(&state.database, &database_staging_path).await {
            Ok(tables) => tables,
            Err(_) => {
                let _ = tokio::fs::remove_dir_all(&database_staging_path).await;
                return Err(ApiError::Internal);
            }
        }
    } else {
        Vec::new()
    };
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
    let archive_manifest = BackupManifest {
        server_version: manifest.server_version.clone(),
        backup_engine_version: manifest.backup_engine_version.clone(),
        date_created: manifest.date_created,
        database_tables: database_tables
            .iter()
            .map(|table| format!("{}.{}", table.schema, table.name))
            .collect(),
        options: manifest.options.clone(),
    };
    let program_data_directory = state.program_data_directory.clone();
    let internal_metadata_directory = state.internal_metadata_directory.clone();
    let result = tokio::task::spawn_blocking(move || {
        let result = create_backup_archive_blocking(
            &temporary_path,
            &archive_manifest,
            &program_data_directory,
            &internal_metadata_directory,
            (!database_tables.is_empty()).then_some(database_staging_path.as_path()),
        )
        .and_then(|()| fs::rename(&temporary_path, &archive_path));
        let _ = fs::remove_dir_all(&database_staging_path);
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

fn parse_create_options(body: &[u8]) -> Result<BackupOptionsDto, ApiError> {
    if body.is_empty() {
        return Ok(BackupOptionsDto::default());
    }
    serde_json::from_slice::<Option<CaseInsensitiveBackupOptions>>(body)
        .map(|options| options.map_or_else(BackupOptionsDto::default, |options| options.0))
        .map_err(|_| ApiError::InvalidRequest)
}

pub(crate) async fn manifest(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let path = manifest_query_path(raw_query.as_deref()).ok_or(ApiError::InvalidRequest)?;
    let Some(archive_path) = sanitized_backup_path(&state, &path) else {
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
    request: Result<Json<CaseInsensitiveBackupRestoreRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;

    let Json(CaseInsensitiveBackupRestoreRequest(request)) =
        request.map_err(|_| ApiError::InvalidRequest)?;
    let Some(archive_path) = sanitized_backup_path(&state, &request.archive_file_name) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if !archive_path.is_file() {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    let validation_path = archive_path.clone();
    let validation =
        tokio::task::spawn_blocking(move || validate_archive_blocking(&validation_path))
            .await
            .map_err(|_| ApiError::Internal)?;
    match validation {
        Ok(manifest) => {
            if validate_database_table_names(&manifest).is_err() {
                return Ok(status_message(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "backup database format is not compatible with the PostgreSQL restore engine",
                ));
            }
            (state.system_command)(crate::SystemCommand::Restore(archive_path));
            Ok(StatusCode::NO_CONTENT.into_response())
        }
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
    manifest: &BackupManifest,
    program_data_directory: &Path,
    internal_metadata_directory: &Path,
    database_staging_directory: Option<&Path>,
) -> std::io::Result<()> {
    let mut archive_options = fs::OpenOptions::new();
    archive_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        archive_options.mode(0o600);
    }
    let file = archive_options.open(path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let directory_options = SimpleFileOptions::default().unix_permissions(0o700);

    archive.add_directory("Data/", directory_options)?;
    let mut excluded = vec![
        program_data_directory.join("backups"),
        program_data_directory.join("metadata"),
        program_data_directory.join("trickplay"),
        program_data_directory.join("subtitles"),
    ];
    if internal_metadata_directory.starts_with(program_data_directory) {
        excluded.push(internal_metadata_directory.to_owned());
    }
    add_directory_tree(
        &mut archive,
        program_data_directory,
        "Data",
        &excluded,
        options,
        directory_options,
    )?;

    if let Some(database_staging_directory) = database_staging_directory {
        archive.add_directory("Database/", directory_options)?;
        add_directory_tree(
            &mut archive,
            database_staging_directory,
            "Database",
            &[],
            options,
            directory_options,
        )?;
    }

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

async fn export_database(
    database: &jellyfin_data::SharedDatabase,
    staging_directory: &Path,
) -> Result<Vec<DatabaseBackupTable>, sea_orm::DbErr> {
    tokio::fs::create_dir(staging_directory)
        .await
        .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
    let transaction = database
        .begin_with_config(
            Some(IsolationLevel::RepeatableRead),
            Some(AccessMode::ReadOnly),
        )
        .await?;
    let rows = transaction
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT table_schema, table_name
            FROM information_schema.tables
            WHERE table_type = 'BASE TABLE'
              AND (table_schema = 'jellyfin'
                   OR (table_schema = 'public' AND table_name = 'seaql_migrations'))
            ORDER BY table_schema, table_name
            "
            .to_owned(),
        ))
        .await?;
    let mut tables = Vec::with_capacity(rows.len());
    for row in rows {
        let schema: String = row.try_get("", "table_schema")?;
        let name: String = row.try_get("", "table_name")?;
        if !portable_database_file_stem(&name) {
            return Err(sea_orm::DbErr::Custom(format!(
                "database table name is not safe for a portable backup: {schema}.{name}"
            )));
        }
        let output_path = staging_directory.join(format!("{name}.json"));
        let mut output = tokio::io::BufWriter::new(
            tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output_path)
                .await
                .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?,
        );
        output
            .write_all(b"[")
            .await
            .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
        let statement = Statement::from_string(
            DbBackend::Postgres,
            format!(
                "SELECT row_to_json(item)::text AS row_json FROM {}.{} AS item ORDER BY ctid",
                quote_postgres_identifier(&schema),
                quote_postgres_identifier(&name),
            ),
        );
        let mut stream = transaction.stream(statement).await?;
        let mut first = true;
        while let Some(row) = stream.try_next().await? {
            let json: String = row.try_get("", "row_json")?;
            if !first {
                output
                    .write_all(b",")
                    .await
                    .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
            }
            output
                .write_all(json.as_bytes())
                .await
                .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
            first = false;
        }
        drop(stream);
        output
            .write_all(b"]")
            .await
            .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
        output
            .flush()
            .await
            .map_err(|error| sea_orm::DbErr::Custom(error.to_string()))?;
        tables.push(DatabaseBackupTable { schema, name });
    }
    transaction.commit().await?;
    Ok(tables)
}

fn portable_database_file_stem(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn quote_postgres_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// Restores a previously validated Rust PostgreSQL backup before the HTTP host starts.
///
/// The caller must ensure no application services are using `database`. Database
/// replacement is one transaction; staged file writes complete before that
/// transaction commits, so an extraction/write failure leaves PostgreSQL unchanged.
///
/// # Errors
///
/// Returns an error for an invalid/incompatible archive, a database failure, or
/// an unsafe/unwritable file target.
pub async fn restore_backup_at_startup(
    database: &DatabaseConnection,
    archive_path: &Path,
    program_data_directory: &Path,
    internal_metadata_directory: &Path,
) -> std::io::Result<()> {
    let archive_path = archive_path.to_owned();
    let staging_directory = program_data_directory
        .join("backups")
        .join(format!(".restore-{}", Uuid::new_v4().simple()));
    let extraction_path = staging_directory.clone();
    let manifest = tokio::task::spawn_blocking(move || {
        stage_restore_archive_blocking(&archive_path, &extraction_path)
    })
    .await
    .map_err(std::io::Error::other)??;

    let result = restore_staged_backup(
        database,
        &manifest,
        &staging_directory,
        program_data_directory,
        internal_metadata_directory,
    )
    .await;
    let cleanup_result = tokio::fs::remove_dir_all(&staging_directory).await;
    match (result, cleanup_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        (Ok(()), _) => Ok(()),
    }
}

async fn restore_staged_backup(
    database: &DatabaseConnection,
    manifest: &BackupManifest,
    staging_directory: &Path,
    program_data_directory: &Path,
    internal_metadata_directory: &Path,
) -> std::io::Result<()> {
    let transaction = if manifest.options.database {
        let transaction = database
            .begin_with_config(
                Some(IsolationLevel::Serializable),
                Some(AccessMode::ReadWrite),
            )
            .await
            .map_err(std::io::Error::other)?;
        load_database_restore(
            &transaction,
            &staging_directory.join("Database"),
            &manifest.database_tables,
        )
        .await?;
        Some(transaction)
    } else {
        None
    };

    let data_directory = staging_directory.join("Data");
    let program_data_directory = program_data_directory.to_owned();
    let internal_metadata_directory = internal_metadata_directory.to_owned();
    tokio::task::spawn_blocking(move || {
        apply_staged_files_blocking(
            &data_directory,
            &program_data_directory,
            &internal_metadata_directory,
        )
    })
    .await
    .map_err(std::io::Error::other)??;

    if let Some(transaction) = transaction {
        transaction.commit().await.map_err(std::io::Error::other)?;
    }
    Ok(())
}

fn stage_restore_archive_blocking(
    archive_path: &Path,
    staging_directory: &Path,
) -> std::io::Result<BackupManifest> {
    let manifest = validate_archive_blocking(archive_path).map_err(|error| match error {
        ArchiveValidationError::Invalid(message) => {
            std::io::Error::new(std::io::ErrorKind::InvalidData, message)
        }
        ArchiveValidationError::Unsupported(message) => {
            std::io::Error::new(std::io::ErrorKind::Unsupported, message)
        }
    })?;
    validate_database_table_names(&manifest)?;
    fs::create_dir(&staging_directory)?;
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).map_err(std::io::Error::other)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(std::io::Error::other)?;
        let Some(enclosed_name) = entry.enclosed_name() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsafe ZIP entry path",
            ));
        };
        if enclosed_name == Path::new(MANIFEST_ENTRY_NAME) || entry.is_dir() {
            continue;
        }
        if !enclosed_name.starts_with("Data") && !enclosed_name.starts_with("Database") {
            continue;
        }
        let output_path = staging_directory.join(&enclosed_name);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_path)?;
        std::io::copy(&mut entry, &mut output)?;
        output.sync_all()?;
    }
    Ok(manifest)
}

fn validate_database_table_names(manifest: &BackupManifest) -> std::io::Result<()> {
    if !manifest.options.database {
        return Ok(());
    }
    if manifest.database_tables.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "database backup contains no table manifest",
        ));
    }
    let mut names = HashSet::new();
    for table in &manifest.database_tables {
        let Some((schema, name)) = table.split_once('.') else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid database table name: {table}"),
            ));
        };
        if !portable_database_file_stem(name)
            || (schema != "jellyfin" && !(schema == "public" && name == "seaql_migrations"))
            || !names.insert(table)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid database table name: {table}"),
            ));
        }
    }
    Ok(())
}

async fn load_database_restore(
    transaction: &DatabaseTransaction,
    database_directory: &Path,
    archived_table_names: &[String],
) -> std::io::Result<()> {
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [BACKUP_DATABASE_LOCK_KEY.into()],
        ))
        .await
        .map_err(std::io::Error::other)?;
    let tables = database_tables(transaction)
        .await
        .map_err(std::io::Error::other)?;
    let current_names = tables
        .iter()
        .map(|table| format!("{}.{}", table.schema, table.name))
        .collect::<HashSet<_>>();
    let archived_names = archived_table_names.iter().cloned().collect::<HashSet<_>>();
    if current_names != archived_names {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "backup database tables do not match the running server schema",
        ));
    }

    let qualified_tables = tables
        .iter()
        .map(|table| {
            format!(
                "{}.{}",
                quote_postgres_identifier(&table.schema),
                quote_postgres_identifier(&table.name)
            )
        })
        .collect::<Vec<_>>();
    transaction
        .execute_unprepared(&format!(
            "LOCK TABLE {} IN ACCESS EXCLUSIVE MODE",
            qualified_tables.join(", ")
        ))
        .await
        .map_err(std::io::Error::other)?;

    let foreign_keys = database_foreign_keys(transaction).await?;
    for foreign_key in &foreign_keys {
        transaction
            .execute_unprepared(&format!(
                "ALTER TABLE {}.{} DROP CONSTRAINT {}",
                quote_postgres_identifier(&foreign_key.schema),
                quote_postgres_identifier(&foreign_key.table),
                quote_postgres_identifier(&foreign_key.name),
            ))
            .await
            .map_err(std::io::Error::other)?;
    }
    transaction
        .execute_unprepared(&format!(
            "TRUNCATE TABLE {} RESTART IDENTITY",
            qualified_tables.join(", ")
        ))
        .await
        .map_err(std::io::Error::other)?;

    for table in &tables {
        let columns = insertable_database_columns(transaction, table).await?;
        load_database_table_batches(transaction, database_directory, table, &columns).await?;
    }
    for foreign_key in foreign_keys {
        transaction
            .execute_unprepared(&format!(
                "ALTER TABLE {}.{} ADD CONSTRAINT {} {}",
                quote_postgres_identifier(&foreign_key.schema),
                quote_postgres_identifier(&foreign_key.table),
                quote_postgres_identifier(&foreign_key.name),
                foreign_key.definition,
            ))
            .await
            .map_err(std::io::Error::other)?;
    }
    reset_database_sequences(transaction).await
}

async fn database_tables(
    connection: &impl ConnectionTrait,
) -> Result<Vec<DatabaseBackupTable>, sea_orm::DbErr> {
    connection
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT table_schema, table_name
            FROM information_schema.tables
            WHERE table_type = 'BASE TABLE'
              AND (table_schema = 'jellyfin'
                   OR (table_schema = 'public' AND table_name = 'seaql_migrations'))
            ORDER BY table_schema, table_name
            "
            .to_owned(),
        ))
        .await?
        .into_iter()
        .map(|row| {
            Ok(DatabaseBackupTable {
                schema: row.try_get("", "table_schema")?,
                name: row.try_get("", "table_name")?,
            })
        })
        .collect()
}

struct DatabaseForeignKey {
    schema: String,
    table: String,
    name: String,
    definition: String,
}

async fn database_foreign_keys(
    transaction: &DatabaseTransaction,
) -> std::io::Result<Vec<DatabaseForeignKey>> {
    transaction
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT namespace.nspname AS table_schema,
                   relation.relname AS table_name,
                   con.conname AS constraint_name,
                   pg_get_constraintdef(con.oid) AS definition
            FROM pg_constraint AS con
            JOIN pg_class AS relation ON relation.oid = con.conrelid
            JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
            WHERE con.contype = 'f' AND namespace.nspname = 'jellyfin'
            ORDER BY namespace.nspname, relation.relname, con.conname
            "
            .to_owned(),
        ))
        .await
        .map_err(std::io::Error::other)?
        .into_iter()
        .map(|row| {
            Ok(DatabaseForeignKey {
                schema: row
                    .try_get("", "table_schema")
                    .map_err(std::io::Error::other)?,
                table: row
                    .try_get("", "table_name")
                    .map_err(std::io::Error::other)?,
                name: row
                    .try_get("", "constraint_name")
                    .map_err(std::io::Error::other)?,
                definition: row
                    .try_get("", "definition")
                    .map_err(std::io::Error::other)?,
            })
        })
        .collect()
}

async fn insertable_database_columns(
    transaction: &DatabaseTransaction,
    table: &DatabaseBackupTable,
) -> std::io::Result<Vec<String>> {
    transaction
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            SELECT column_name
            FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = $2 AND is_generated = 'NEVER'
            ORDER BY ordinal_position
            ",
            [table.schema.clone().into(), table.name.clone().into()],
        ))
        .await
        .map_err(std::io::Error::other)?
        .into_iter()
        .map(|row| {
            row.try_get("", "column_name")
                .map_err(std::io::Error::other)
        })
        .collect()
}

async fn load_database_table_batches(
    transaction: &DatabaseTransaction,
    database_directory: &Path,
    table: &DatabaseBackupTable,
    columns: &[String],
) -> std::io::Result<()> {
    let input_path = database_directory.join(format!("{}.json", table.name));
    if !input_path.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("database backup is missing {}.json", table.name),
        ));
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    let parser = tokio::task::spawn_blocking(move || parse_json_array_batches(&input_path, sender));
    let quoted_columns = columns
        .iter()
        .map(|column| quote_postgres_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {}.{} ({quoted_columns}) SELECT {quoted_columns} FROM jsonb_populate_recordset(NULL::{}.{}, $1::jsonb)",
        quote_postgres_identifier(&table.schema),
        quote_postgres_identifier(&table.name),
        quote_postgres_identifier(&table.schema),
        quote_postgres_identifier(&table.name),
    );
    while let Some(batch) = receiver.recv().await {
        let json = serde_json::to_string(&batch).map_err(std::io::Error::other)?;
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                &sql,
                [json.into()],
            ))
            .await
            .map_err(std::io::Error::other)?;
    }
    parser.await.map_err(std::io::Error::other)??;
    Ok(())
}

struct JsonArrayBatchSeed {
    sender: tokio::sync::mpsc::Sender<Vec<serde_json::Value>>,
}

impl<'de> DeserializeSeed<'de> for JsonArrayBatchSeed {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(JsonArrayBatchVisitor {
            sender: self.sender,
        })
    }
}

struct JsonArrayBatchVisitor {
    sender: tokio::sync::mpsc::Sender<Vec<serde_json::Value>>,
}

impl<'de> de::Visitor<'de> for JsonArrayBatchVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON array of PostgreSQL rows")
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut batch = Vec::with_capacity(DATABASE_RESTORE_BATCH_SIZE);
        while let Some(value) = sequence.next_element()? {
            batch.push(value);
            if batch.len() == DATABASE_RESTORE_BATCH_SIZE {
                self.sender
                    .blocking_send(std::mem::take(&mut batch))
                    .map_err(de::Error::custom)?;
            }
        }
        if !batch.is_empty() {
            self.sender
                .blocking_send(batch)
                .map_err(de::Error::custom)?;
        }
        Ok(())
    }
}

fn parse_json_array_batches(
    path: &Path,
    sender: tokio::sync::mpsc::Sender<Vec<serde_json::Value>>,
) -> std::io::Result<()> {
    let input = File::open(path)?;
    let mut deserializer = serde_json::Deserializer::from_reader(std::io::BufReader::new(input));
    JsonArrayBatchSeed { sender }
        .deserialize(&mut deserializer)
        .map_err(std::io::Error::other)?;
    deserializer.end().map_err(std::io::Error::other)
}

async fn reset_database_sequences(transaction: &DatabaseTransaction) -> std::io::Result<()> {
    let rows = transaction
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            r"
            SELECT table_schema, table_name, column_name
            FROM information_schema.columns
            WHERE table_schema = 'jellyfin'
              AND (is_identity = 'YES' OR column_default LIKE 'nextval(%')
            ORDER BY table_schema, table_name, ordinal_position
            "
            .to_owned(),
        ))
        .await
        .map_err(std::io::Error::other)?;
    for row in rows {
        let schema: String = row
            .try_get("", "table_schema")
            .map_err(std::io::Error::other)?;
        let table: String = row
            .try_get("", "table_name")
            .map_err(std::io::Error::other)?;
        let column: String = row
            .try_get("", "column_name")
            .map_err(std::io::Error::other)?;
        let relation = format!("{schema}.{table}");
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "SELECT setval(pg_get_serial_sequence($1, $2), COALESCE(MAX({}), 1), MAX({}) IS NOT NULL) FROM {}.{}",
                    quote_postgres_identifier(&column),
                    quote_postgres_identifier(&column),
                    quote_postgres_identifier(&schema),
                    quote_postgres_identifier(&table),
                ),
                [relation.into(), column.into()],
            ))
            .await
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

fn apply_staged_files_blocking(
    data_directory: &Path,
    program_data_directory: &Path,
    internal_metadata_directory: &Path,
) -> std::io::Result<()> {
    if !data_directory.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "backup contains no staged Data directory",
        ));
    }
    let mut pending = vec![data_directory.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "staged backup contains a symbolic link",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "staged backup contains an unsupported file type",
                ));
            }
            let relative = path
                .strip_prefix(data_directory)
                .map_err(std::io::Error::other)?;
            let (root, relative) = match relative.components().next() {
                Some(Component::Normal(component)) if component == "metadata" => (
                    internal_metadata_directory,
                    relative
                        .strip_prefix("metadata")
                        .map_err(std::io::Error::other)?,
                ),
                _ => (program_data_directory, relative),
            };
            atomic_restore_file(&path, root, relative)?;
        }
    }
    Ok(())
}

fn atomic_restore_file(source: &Path, root: &Path, relative: &Path) -> std::io::Result<()> {
    let mut destination_parent = root.to_owned();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid staged restore path",
            ));
        };
        if components.peek().is_none() {
            let destination = destination_parent.join(component);
            if fs::symlink_metadata(&destination)
                .ok()
                .is_some_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "refusing to replace a symbolic link during restore",
                ));
            }
            fs::create_dir_all(&destination_parent)?;
            let temporary = destination_parent.join(format!(
                ".{}.restore-{}.partial",
                component.to_string_lossy(),
                Uuid::new_v4().simple()
            ));
            let result = (|| {
                let mut input = File::open(source)?;
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                fs::rename(&temporary, destination)
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            return result;
        }
        destination_parent.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&destination_parent) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unsafe restore destination ancestor",
                ));
            }
        }
    }
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

fn validate_archive_blocking(path: &Path) -> Result<BackupManifest, ArchiveValidationError> {
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
    Ok(manifest)
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

fn manifest_query_path(raw_query: Option<&str>) -> Option<String> {
    form_urlencoded::parse(raw_query?.as_bytes())
        .filter(|(key, _)| key.eq_ignore_ascii_case("path"))
        .map(|(_, value)| value.into_owned())
        .last()
}

fn sanitized_backup_path(state: &AppState, path: &str) -> Option<PathBuf> {
    let requested = Path::new(path);
    let backup_directory = state.program_data_directory.join("backups");
    // The official controller reduces both the absolute path returned by
    // ListBackups and client-supplied paths to a file name. Never retain a
    // caller-controlled parent directory when joining the backup root.
    let file_name = requested.file_name()?.to_str()?;
    let candidate = backup_directory.join(file_name);
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

#[cfg(test)]
mod tests {
    use super::{
        CaseInsensitiveBackupRestoreRequest, manifest_query_path, parse_create_options,
        quote_postgres_identifier,
    };

    #[test]
    fn create_options_default_only_for_omitted_or_null_bodies() {
        for body in [
            b"".as_slice(),
            b"null".as_slice(),
            b" \n null \t".as_slice(),
        ] {
            let options = parse_create_options(body).unwrap();
            assert!(options.database);
            assert!(!options.metadata);
            assert!(!options.trickplay);
            assert!(!options.subtitles);
        }

        let options = parse_create_options(
            br#"{"metadata":true,"TRICKPLAY":true,"SubTitles":true,"database":true,"DATABASE":false}"#,
        )
        .unwrap();
        assert!(!options.database);
        assert!(options.metadata);
        assert!(options.trickplay);
        assert!(options.subtitles);
    }

    #[test]
    fn manifest_query_name_is_case_insensitive_and_last_value_wins() {
        assert_eq!(
            manifest_query_path(Some("PATH=first.zip&pAtH=second.zip")),
            Some("second.zip".to_owned())
        );
        assert_eq!(manifest_query_path(Some("other=value")), None);
    }

    #[test]
    fn restore_body_is_case_insensitive_and_last_value_wins() {
        let CaseInsensitiveBackupRestoreRequest(request) = serde_json::from_str(
            r#"{"archivefilename":"first.zip","ARCHIVEFILENAME":"second.zip"}"#,
        )
        .unwrap();
        assert_eq!(request.archive_file_name, "second.zip");
    }

    #[test]
    fn postgres_identifiers_are_quoted_without_interpolation() {
        assert_eq!(quote_postgres_identifier("ordinary"), "\"ordinary\"");
        assert_eq!(quote_postgres_identifier("odd\"name"), "\"odd\"\"name\"");
    }

    #[test]
    fn create_options_reject_malformed_json_and_wrong_types() {
        for body in [
            b"{".as_slice(),
            br#""backup""#.as_slice(),
            b"[]".as_slice(),
            br#"{"Database":"false"}"#.as_slice(),
        ] {
            assert!(parse_create_options(body).is_err());
        }
    }
}
