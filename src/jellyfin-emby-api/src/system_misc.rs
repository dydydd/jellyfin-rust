//! Small Emby compatibility endpoints whose Jellyfin equivalents are not
//! exposed as reusable public handlers.

use std::sync::Arc;

use axum::{Json, Router, http::StatusCode, routing::get};
use jellyfin_api::AppState;
use serde::Serialize;

/// Emby system/discovery routes.  The parent router supplies the state.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/System/Ping", get(ping).post(ping).head(ping))
        .route("/system/ping", get(ping).post(ping).head(ping))
        .route("/System/WakeOnLanInfo", get(wake_on_lan_info))
        .route("/system/wakeonlaninfo", get(wake_on_lan_info))
        .route("/System/ReleaseNotes", get(release_notes))
        .route("/system/releasenotes", get(release_notes))
        .route("/System/ReleaseNotes/Versions", get(release_note_versions))
        .route("/system/releasenotes/versions", get(release_note_versions))
        .route("/Packages/Updates", get(package_updates))
        .route("/packages/updates", get(package_updates))
        .route("/Shows/Missing", get(empty_items))
        .route("/shows/missing", get(empty_items))
        .route("/AudioBooks/NextUp", get(empty_items))
        .route("/audiobooks/nextup", get(empty_items))
        .route("/StreamLanguages", get(stream_languages))
        .route("/streamlanguages", get(stream_languages))
        .route("/SubtitleCodecs", get(subtitle_codecs))
        .route("/subtitlecodecs", get(subtitle_codecs))
        .route("/VideoCodecs", get(video_codecs))
        .route("/videocodecs", get(video_codecs))
        .route("/Tags", get(tags))
        .route("/tags", get(tags))
}

async fn ping() -> StatusCode {
    // Emby's generated clients model Ping as `void`, with a successful 200.
    StatusCode::OK
}

// The server does not expose Wake-on-LAN configuration.  An empty list is the
// valid Emby response when no Wake-on-LAN devices are configured.
async fn wake_on_lan_info() -> Json<Vec<()>> {
    Json(Vec::new())
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PackageVersionInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version_str: Option<String>,
}

async fn release_notes() -> Json<PackageVersionInfo> {
    Json(PackageVersionInfo {
        name: None,
        version_str: None,
    })
}

async fn release_note_versions() -> Json<Vec<PackageVersionInfo>> {
    Json(Vec::new())
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult<T> {
    items: Vec<T>,
    total_record_count: usize,
}

async fn package_updates() -> Json<Vec<PackageVersionInfo>> {
    Json(Vec::new())
}

async fn empty_items() -> Json<QueryResult<()>> {
    Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
    })
}

async fn stream_languages() -> Json<QueryResult<String>> {
    strings()
}
async fn subtitle_codecs() -> Json<QueryResult<String>> {
    strings()
}
async fn video_codecs() -> Json<QueryResult<String>> {
    strings()
}
async fn tags() -> Json<QueryResult<String>> {
    strings()
}

fn strings() -> Json<QueryResult<String>> {
    Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn mobile_discovery_routes_are_reachable() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        for path in [
            "/System/ReleaseNotes",
            "/System/ReleaseNotes/Versions",
            "/System/WakeOnLanInfo",
            "/Packages/Updates",
            "/Shows/Missing",
            "/AudioBooks/NextUp",
            "/StreamLanguages",
            "/SubtitleCodecs",
            "/VideoCodecs",
            "/Tags",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(response.status().is_success(), "{path}");
            let _ = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        }
    }

    #[tokio::test]
    async fn ping_is_an_empty_success_response() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        let response = app
            .oneshot(Request::get("/System/Ping").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
