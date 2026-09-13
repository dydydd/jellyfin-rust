//! Emby's backup discovery contract, backed by Jellyfin's persisted archives.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/BackupRestore/BackupInfo", get(backup_info))
        .route("/backuprestore/backupinfo", get(backup_info))
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

async fn backup_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<BackupInfo>, Response> {
    let manifests = jellyfin_api::backup::emby_list(State(state), OriginalUri(uri), headers)
        .await
        .map_err(|error| error.into_response())?
        .0;
    let mut backups = manifests
        .into_iter()
        .map(|manifest| Backup {
            server_version: manifest.server_version,
            plugin_version: None,
            name: std::path::Path::new(&manifest.path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned(),
            can_restore: false,
            is_full_backup: true,
            date_created: manifest.date_created,
            users: None,
        })
        .collect::<Vec<_>>();
    let full_backup_info = backups.pop();
    Ok(Json(BackupInfo {
        full_backup_info,
        light_backups: backups,
    }))
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
}
