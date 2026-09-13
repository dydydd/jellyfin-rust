//! Small Emby library-discovery endpoints that are not part of Jellyfin's
//! public route set.
//!
//! These endpoints are intentionally data-only.  The actual item browsing,
//! filtering, DTO projection, and authorization remain in the shared
//! Jellyfin router mounted as the fallback by `crate::router`.

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use jellyfin_api::AppState;
use serde::Serialize;

/// Routes owned by the Emby protocol surface.
///
/// The caller supplies the state to the final router with `with_state`; this
/// fragment deliberately does not do so, allowing it to be merged before the
/// shared fallback.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/Prefixes", get(item_prefixes))
        .route("/items/prefixes", get(item_prefixes))
        .route("/Artists/Prefixes", get(artist_prefixes))
        .route("/artists/prefixes", get(artist_prefixes))
        .route("/ItemTypes", get(item_types))
        .route("/itemtypes", get(item_types))
        .route("/AudioCodecs", get(audio_codecs))
        .route("/audiocodecs", get(audio_codecs))
        .route("/AudioLayouts", get(audio_layouts))
        .route("/audiolayouts", get(audio_layouts))
        .route("/Containers", get(containers))
        .route("/containers", get(containers))
        .route("/ExtendedVideoTypes", get(extended_video_types))
        .route("/extendedvideotypes", get(extended_video_types))
        .route("/OfficialRatings", get(official_ratings))
        .route("/officialratings", get(official_ratings))
        .route("/Features", get(features))
        .route("/features", get(features))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct NameValuePair {
    name: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TagResult {
    total_record_count: usize,
    items: Vec<NameValuePair>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ItemType {
    id: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ItemTypeResult {
    total_record_count: usize,
    items: Vec<ItemType>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FeatureInfo {
    feature_type: String,
    id: String,
    name: String,
}

async fn item_prefixes(State(_state): State<Arc<AppState>>) -> Json<Vec<NameValuePair>> {
    Json(Vec::new())
}

async fn artist_prefixes(State(_state): State<Arc<AppState>>) -> Json<Vec<NameValuePair>> {
    Json(Vec::new())
}

async fn item_types(State(_state): State<Arc<AppState>>) -> Json<ItemTypeResult> {
    let items = [
        "Audio",
        "AudioBook",
        "Book",
        "Episode",
        "Folder",
        "Genre",
        "Movie",
        "MusicAlbum",
        "MusicArtist",
        "MusicVideo",
        "Person",
        "Photo",
        "Playlist",
        "Season",
        "Series",
        "Studio",
        "Trailer",
        "Video",
    ]
    .into_iter()
    .map(|name| ItemType {
        id: name.to_owned(),
        name: name.to_owned(),
    })
    .collect::<Vec<_>>();
    Json(ItemTypeResult {
        total_record_count: items.len(),
        items,
    })
}

// The server's codec/container registry is not exposed by AppState.  Empty
// arrays are the official no-capability result and are preferable to claiming
// support for a codec that a deployment cannot actually play.
async fn audio_codecs(State(_state): State<Arc<AppState>>) -> Json<TagResult> {
    Json(TagResult {
        total_record_count: 0,
        items: Vec::new(),
    })
}

async fn audio_layouts(State(_state): State<Arc<AppState>>) -> Json<TagResult> {
    Json(TagResult {
        total_record_count: 0,
        items: Vec::new(),
    })
}

async fn containers(State(_state): State<Arc<AppState>>) -> Json<TagResult> {
    Json(TagResult {
        total_record_count: 0,
        items: Vec::new(),
    })
}

async fn extended_video_types(State(_state): State<Arc<AppState>>) -> Json<TagResult> {
    Json(TagResult {
        total_record_count: 0,
        items: Vec::new(),
    })
}

async fn official_ratings(State(_state): State<Arc<AppState>>) -> Json<TagResult> {
    Json(TagResult {
        total_record_count: 0,
        items: Vec::new(),
    })
}

async fn features(State(_state): State<Arc<AppState>>) -> Json<Vec<FeatureInfo>> {
    Json(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn discovery_routes_keep_emby_array_shapes() {
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let app = routes().with_state(Arc::new(state));
        let response = app
            .oneshot(Request::get("/ItemTypes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["TotalRecordCount"], 18);
        assert_eq!(value["Items"][0]["Id"], "Audio");
    }
}
