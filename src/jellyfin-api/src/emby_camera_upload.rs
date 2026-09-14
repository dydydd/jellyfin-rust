//! Internal storage and request identity for Emby's retired camera-upload API.

use std::path::Path;

use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{Stream, StreamExt, pin_mut};
use jellyfin_data::{
    EmbyCameraUploadStoreError, NewEmbyCameraUpload, entities::emby_camera_upload,
};
use md5::{Digest, Md5};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization};

const MAX_CAMERA_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 512;
const MAX_ALBUM_CHARS: usize = 512;
const MAX_UPLOAD_ID_CHARS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbyCameraDeviceContext {
    pub device_id: String,
    pub device_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbyCameraUploadMetadata {
    pub album: String,
    pub name: String,
    pub upload_id: String,
    pub mime_type: Option<String>,
    pub date_created: Option<DateTime<Utc>>,
}

impl AppState {
    /// Resolves the currently authenticated reported device. Device sessions
    /// use persisted metadata; official user-less API keys use their request
    /// authorization header after the shared token check succeeds.
    ///
    /// # Errors
    ///
    /// Returns an authentication, policy, or malformed API-key device-header
    /// response when no current reported device can be resolved.
    pub async fn emby_camera_device_context(
        &self,
        headers: &HeaderMap,
        uri: &Uri,
    ) -> Result<EmbyCameraDeviceContext, Response> {
        let identity = authorization::require_default(self, headers, uri)
            .await
            .map_err(IntoResponse::into_response)?;
        match identity {
            authentication::AuthenticatedIdentity::Device(session) => Ok(EmbyCameraDeviceContext {
                device_id: session.device.device_id,
                device_name: session.device.device_name,
            }),
            authentication::AuthenticatedIdentity::ApiKey(_) => {
                let request = authentication::authorization_info_from_headers(headers)
                    .map_err(IntoResponse::into_response)?;
                Ok(EmbyCameraDeviceContext {
                    device_id: request.device_id,
                    device_name: request.device_name,
                })
            }
        }
    }

    /// Loads the append-only history for the resolved reported device.
    ///
    /// # Errors
    ///
    /// Returns a validation or database response when history cannot be read.
    pub async fn emby_camera_upload_history(
        &self,
        context: &EmbyCameraDeviceContext,
    ) -> Result<Vec<emby_camera_upload::Model>, Response> {
        self.emby_camera_uploads
            .list(&context.device_id)
            .await
            .map_err(camera_store_error_response)
    }

    /// Streams one upload into a same-directory temporary file, publishes it
    /// with an atomic rename, then appends its PostgreSQL history row. The
    /// unique target permits safe cleanup if persistence fails.
    ///
    /// # Errors
    ///
    /// Returns a client response for invalid metadata, malformed/oversized
    /// streams, unsafe storage paths, filesystem errors, or database failures.
    pub async fn accept_emby_camera_upload<S, E>(
        &self,
        context: &EmbyCameraDeviceContext,
        metadata: EmbyCameraUploadMetadata,
        stream: S,
    ) -> Result<(), Response>
    where
        S: Stream<Item = Result<Bytes, E>>,
        E: std::fmt::Display,
    {
        validate_metadata(&metadata)?;
        let directory = safe_upload_directory(
            &self.program_data_directory.join("camera-uploads"),
            context,
            &metadata.album,
        )
        .await?;
        let extension = upload_extension(&metadata.name, metadata.mime_type.as_deref());
        let safe_name = sanitized_component(&metadata.name, 160);
        let stem = Path::new(&safe_name)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("upload");
        let nonce = Uuid::new_v4().simple();
        let target = directory.join(format!("{stem}-{nonce}.{extension}"));
        let temporary = directory.join(format!(".{nonce}.upload"));

        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(internal_io_response)?;
        let mut total = 0_u64;
        pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    drop(file);
                    let _ = fs::remove_file(&temporary).await;
                    tracing::debug!(%error, "camera upload request stream failed");
                    return Err(StatusCode::BAD_REQUEST.into_response());
                }
            };
            total = total.saturating_add(chunk.len() as u64);
            if total > MAX_CAMERA_UPLOAD_BYTES {
                drop(file);
                let _ = fs::remove_file(&temporary).await;
                return Err(StatusCode::PAYLOAD_TOO_LARGE.into_response());
            }
            if let Err(error) = file.write_all(&chunk).await {
                drop(file);
                let _ = fs::remove_file(&temporary).await;
                return Err(internal_io_response(error));
            }
        }
        if total == 0 {
            drop(file);
            let _ = fs::remove_file(&temporary).await;
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
        if let Err(error) = file.sync_all().await {
            drop(file);
            let _ = fs::remove_file(&temporary).await;
            return Err(internal_io_response(error));
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, &target).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(internal_io_response(error));
        }

        let record = NewEmbyCameraUpload {
            device_id: context.device_id.clone(),
            upload_id: metadata.upload_id,
            name: metadata.name,
            album: metadata.album,
            mime_type: metadata.mime_type,
            date_created: metadata.date_created,
            stored_path: target.to_string_lossy().into_owned(),
        };
        if let Err(error) = self.emby_camera_uploads.insert(record).await {
            if let Err(cleanup_error) = fs::remove_file(&target).await {
                tracing::warn!(%cleanup_error, path = %target.display(), "failed to clean camera upload after database error");
            }
            return Err(camera_store_error_response(error));
        }
        Ok(())
    }
}

fn validate_metadata(metadata: &EmbyCameraUploadMetadata) -> Result<(), Response> {
    for (value, max, allow_blank) in [
        (metadata.album.as_str(), MAX_ALBUM_CHARS, true),
        (metadata.name.as_str(), MAX_NAME_CHARS, false),
        (metadata.upload_id.as_str(), MAX_UPLOAD_ID_CHARS, false),
    ] {
        if (!allow_blank && value.trim().is_empty()) || value.chars().count() > max {
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
    }
    if metadata
        .mime_type
        .as_deref()
        .is_some_and(|value| value.chars().count() > 255)
    {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    Ok(())
}

async fn safe_upload_directory(
    storage_root: &Path,
    context: &EmbyCameraDeviceContext,
    album: &str,
) -> Result<std::path::PathBuf, Response> {
    fs::create_dir_all(storage_root)
        .await
        .map_err(internal_io_response)?;
    let root = fs::canonicalize(storage_root)
        .await
        .map_err(internal_io_response)?;
    let mut hasher = Md5::new();
    hasher.update(context.device_id.to_lowercase().as_bytes());
    let device_key = format!("{:x}", hasher.finalize());
    let mut directory = root.join(device_key);
    let device_name = sanitized_component(&context.device_name, 80);
    if !device_name.is_empty() {
        directory = directory.join(device_name);
    }
    if !album.trim().is_empty() {
        directory = directory.join(sanitized_component(album, 120));
    }
    fs::create_dir_all(&directory)
        .await
        .map_err(internal_io_response)?;
    let directory = fs::canonicalize(directory)
        .await
        .map_err(internal_io_response)?;
    if !directory.starts_with(&root) {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    Ok(directory)
}

fn sanitized_component(value: &str, max_chars: usize) -> String {
    let mut output = value
        .trim()
        .chars()
        .take(max_chars)
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    while output.starts_with('.') {
        output.remove(0);
    }
    while output.ends_with([' ', '.']) {
        output.pop();
    }
    if output.is_empty() {
        output.push_str("upload");
    }
    output
}

fn upload_extension(name: &str, mime_type: Option<&str>) -> String {
    let mime = mime_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    let known = match mime.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/heic" | "image/heif" => Some("heic"),
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        "video/x-matroska" => Some("mkv"),
        _ => None,
    };
    known.map_or_else(
        || {
            Path::new(name)
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= 10
                        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
                .unwrap_or("jpg")
                .to_ascii_lowercase()
        },
        ToOwned::to_owned,
    )
}

fn camera_store_error_response(error: EmbyCameraUploadStoreError) -> Response {
    match error {
        EmbyCameraUploadStoreError::EmptyField(_)
        | EmbyCameraUploadStoreError::FieldTooLong { .. } => {
            StatusCode::BAD_REQUEST.into_response()
        }
        EmbyCameraUploadStoreError::Database(_) => ApiError::Internal.into_response(),
    }
}

fn internal_io_response(error: std::io::Error) -> Response {
    tracing::warn!(%error, "camera upload storage error");
    ApiError::Internal.into_response()
}

#[cfg(test)]
mod tests {
    use super::{sanitized_component, upload_extension};

    #[test]
    fn names_cannot_escape_the_camera_upload_root() {
        assert_eq!(
            sanitized_component("../../a\\b:name.jpg", 80),
            "_.._a_b_name.jpg"
        );
        assert_eq!(sanitized_component("...", 80), "upload");
        assert_eq!(sanitized_component("  album  ", 80), "album");
    }

    #[test]
    fn mime_type_precedes_a_safe_source_extension() {
        assert_eq!(upload_extension("photo.png", Some("image/jpeg")), "jpg");
        assert_eq!(upload_extension("clip.MP4", None), "mp4");
        assert_eq!(upload_extension("photo.bad/path", None), "jpg");
    }
}
