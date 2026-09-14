//! Emby's backup discovery and safe restore compatibility contract.
//!
//! The historical MBBackup plugin restored a directory tree and copied Emby's
//! SQLite databases into place while the server was shutting down.  Rust
//! backups instead contain a versioned manifest and PostgreSQL table data.
//! Only the default full restore can therefore be delegated to the native
//! restore engine.  SQLite/light-backup and selective user-data restores are
//! rejected explicitly rather than reporting success without restoring data.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::json;
use tower::ServiceExt;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/BackupRestore/BackupInfo", get(backup_info))
        .route("/backuprestore/backupinfo", get(backup_info))
        .route("/BackupRestore/Restore", post(restore))
        .route("/backuprestore/restore", post(restore))
        .route("/BackupRestore/RestoreData", post(restore_data))
        .route("/backuprestore/restoredata", post(restore_data))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct BackupInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    full_backup_info: Option<Backup>,
    light_backups: Vec<Backup>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Backup {
    server_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_version: Option<String>,
    name: String,
    can_restore: bool,
    is_full_backup: bool,
    date_created: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    users: Option<Vec<jellyfin_model::NameIdPair>>,
}

#[derive(Debug, PartialEq, Eq)]
struct RestoreOptions {
    restore_server_id: bool,
    use_files: Option<String>,
}

impl<'de> Deserialize<'de> for RestoreOptions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RestoreOptionsVisitor;

        impl<'de> de::Visitor<'de> for RestoreOptionsVisitor {
            type Value = RestoreOptions;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby backup restore options object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                // MBBackup initializes this property to true. Preserve that
                // default when generated clients omit it.
                let mut restore_server_id = true;
                let mut use_files = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("RestoreServerId") {
                        restore_server_id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("UseFiles") {
                        // The outer option distinguishes an omitted property
                        // from an explicit null while still retaining the last
                        // case-insensitive duplicate.
                        use_files = Some(map.next_value::<Option<String>>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(RestoreOptions {
                    restore_server_id,
                    use_files: use_files.unwrap_or(None),
                })
            }
        }

        deserializer.deserialize_map(RestoreOptionsVisitor)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DataRestoreOptions {
    users: Vec<UserRestoreInfo>,
}

impl<'de> Deserialize<'de> for DataRestoreOptions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DataRestoreOptionsVisitor;

        impl<'de> de::Visitor<'de> for DataRestoreOptionsVisitor {
            type Value = DataRestoreOptions;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby data restore options object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut users = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("Users") {
                        users = Some(map.next_value::<Option<Vec<UserRestoreInfo>>>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(DataRestoreOptions {
                    users: users.flatten().unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_map(DataRestoreOptionsVisitor)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct UserRestoreInfo {
    source_user_id: Option<String>,
    target_user_id: Option<String>,
}

impl<'de> Deserialize<'de> for UserRestoreInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UserRestoreInfoVisitor;

        impl<'de> de::Visitor<'de> for UserRestoreInfoVisitor {
            type Value = UserRestoreInfo;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby user restore mapping object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut source_user_id = None;
                let mut target_user_id = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("SourceUserId") {
                        source_user_id = Some(map.next_value::<Option<String>>()?);
                    } else if name.eq_ignore_ascii_case("TargetUserId") {
                        target_user_id = Some(map.next_value::<Option<String>>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(UserRestoreInfo {
                    source_user_id: source_user_id.flatten(),
                    target_user_id: target_user_id.flatten(),
                })
            }
        }

        deserializer.deserialize_map(UserRestoreInfoVisitor)
    }
}

async fn backup_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<BackupInfo>, Response> {
    let mut manifests = jellyfin_api::backup::emby_list(State(state), OriginalUri(uri), headers)
        .await
        .map_err(|error| error.into_response())?
        .0;
    let full_backup_info = manifests.pop().map(|manifest| Backup {
        server_version: manifest.server_version,
        plugin_version: None,
        name: std::path::Path::new(&manifest.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
        can_restore: true,
        is_full_backup: true,
        date_created: manifest.date_created,
        users: None,
    });
    // Older Rust archives are complete snapshots, not the plugin's
    // database-only "light backups" selected by UseFiles. Advertising them
    // in that collection would cause the dashboard to request a materially
    // different restore operation.
    Ok(Json(BackupInfo {
        full_backup_info,
        light_backups: Vec::new(),
    }))
}

#[allow(clippy::result_large_err)]
async fn restore(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<RestoreOptions>, JsonRejection>,
) -> Result<Response, Response> {
    // Match the generated operation's elevation boundary before exposing JSON
    // binding, archive existence, or restore-engine details.
    state.require_emby_administrator(&headers, &uri).await?;
    let Json(options) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;

    if !options.restore_server_id {
        return Err(unsupported(
            "preserving the current server id is not supported by the PostgreSQL restore engine",
        ));
    }
    if options
        .use_files
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty())
    {
        return Err(unsupported(
            "Emby SQLite light-backup selection is not compatible with PostgreSQL restore",
        ));
    }

    let mut manifests = jellyfin_api::backup::emby_list(
        State(Arc::clone(&state)),
        OriginalUri(uri),
        headers.clone(),
    )
    .await?
    .0;
    let Some(manifest) = manifests.pop() else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };

    // Delegate archive canonicalization, symlink/path traversal rejection,
    // manifest validation, PostgreSQL table-format validation, and host
    // lifecycle scheduling to Jellyfin's native restore endpoint. Building an
    // in-process request keeps those safety checks single-sourced without
    // exposing the Emby route outside /emby.
    let body = serde_json::to_vec(&json!({"ArchiveFileName": manifest.path}))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let mut native_request = Request::post("/Backup/Restore")
        .body(Body::from(body))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    *native_request.headers_mut() = headers;
    native_request.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    let response = jellyfin_api::unprefixed_router(state.as_ref().clone())
        .oneshot(native_request)
        .await
        .map_err(|error| match error {})?;
    if response.status() == StatusCode::NO_CONTENT {
        Ok(StatusCode::OK.into_response())
    } else {
        Ok(response)
    }
}

#[allow(clippy::result_large_err)]
async fn restore_data(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<DataRestoreOptions>, JsonRejection>,
) -> Result<Response, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let Json(options) = request.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    if options.users.is_empty() {
        // This is the same genuine no-op performed by MBBackup for an empty
        // checkbox selection, so a successful empty response is truthful.
        return Ok(StatusCode::OK.into_response());
    }

    // MBBackup's per-user userdata.json contains SQLite-era UserItemData
    // records. Importing it into PostgreSQL would require a dedicated mapping
    // and transaction; a full native archive restore cannot emulate the
    // requested selective copy.
    Err(unsupported(
        "selective Emby user-data restore is not compatible with PostgreSQL backups",
    ))
}

fn unsupported(message: &'static str) -> Response {
    (StatusCode::UNPROCESSABLE_ENTITY, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn backup_info_keeps_canonical_and_lowercase_paths_protected() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        for path in ["/BackupRestore/BackupInfo", "/backuprestore/backupinfo"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    #[test]
    fn restore_bodies_bind_case_insensitively_and_keep_last_duplicate() {
        let options: RestoreOptions = serde_json::from_str(
            r#"{"RestoreServerId":false,"restoreserverid":true,"UseFiles":"old","USEFILES":null,"Ignored":1}"#,
        )
        .unwrap();
        assert_eq!(
            options,
            RestoreOptions {
                restore_server_id: true,
                use_files: None,
            }
        );

        let options: DataRestoreOptions = serde_json::from_str(
            r#"{"Users":[{"SourceUserId":"old"}],"uSeRs":[{"SOURCEUSERID":"source","sourceuserid":"final","TARGETUSERID":"target","Unknown":true}]}"#,
        )
        .unwrap();
        assert_eq!(
            options,
            DataRestoreOptions {
                users: vec![UserRestoreInfo {
                    source_user_id: Some("final".to_owned()),
                    target_user_id: Some("target".to_owned()),
                }],
            }
        );
    }

    #[test]
    fn restore_body_defaults_match_mbbackup_models() {
        assert_eq!(
            serde_json::from_str::<RestoreOptions>("{}").unwrap(),
            RestoreOptions {
                restore_server_id: true,
                use_files: None,
            }
        );
        assert!(
            serde_json::from_str::<DataRestoreOptions>("{}")
                .unwrap()
                .users
                .is_empty()
        );
        assert!(
            serde_json::from_str::<DataRestoreOptions>(r#"{"Users":null}"#)
                .unwrap()
                .users
                .is_empty()
        );
    }
}
