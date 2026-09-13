//! Small Emby library-discovery endpoints that are not part of Jellyfin's
//! public route set.
//!
//! These endpoints are intentionally data-only.  The actual item browsing,
//! filtering, DTO projection, and authorization remain in the shared
//! Jellyfin router mounted as the fallback by `crate::router`.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, Serialize, de};

/// Routes owned by the Emby protocol surface.
///
/// The caller supplies the state to the final router with `with_state`; this
/// fragment deliberately does not do so, allowing it to be merged before the
/// shared fallback.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/Access", post(update_item_access))
        .route("/items/access", post(update_item_access))
        .route("/Items/Intros", get(intro_debug_info))
        .route("/items/intros", get(intro_debug_info))
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

#[derive(Debug)]
struct UpdateUserItemAccess {
    item_ids: Option<Vec<String>>,
    user_ids: Option<Vec<String>>,
    item_access: Option<UserItemShareLevel>,
}

#[derive(Debug)]
struct CaseInsensitiveUpdateUserItemAccess(UpdateUserItemAccess);

impl<'de> Deserialize<'de> for CaseInsensitiveUpdateUserItemAccess {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UpdateVisitor;

        impl<'de> de::Visitor<'de> for UpdateVisitor {
            type Value = CaseInsensitiveUpdateUserItemAccess;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an update user item access object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut item_ids = None;
                let mut user_ids = None;
                let mut item_access = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("ItemIds") {
                        item_ids = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("UserIds") {
                        user_ids = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("ItemAccess") {
                        item_access = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(CaseInsensitiveUpdateUserItemAccess(UpdateUserItemAccess {
                    item_ids,
                    user_ids,
                    item_access,
                }))
            }
        }

        deserializer.deserialize_map(UpdateVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserItemShareLevel {
    None,
    Read,
    Write,
    Manage,
    ManageDelete,
}

impl<'de> Deserialize<'de> for UserItemShareLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ShareLevelVisitor;

        impl de::Visitor<'_> for ShareLevelVisitor {
            type Value = UserItemShareLevel;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby UserItemShareLevel name or integer")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.eq_ignore_ascii_case("None") {
                    Ok(UserItemShareLevel::None)
                } else if value.eq_ignore_ascii_case("Read") {
                    Ok(UserItemShareLevel::Read)
                } else if value.eq_ignore_ascii_case("Write") {
                    Ok(UserItemShareLevel::Write)
                } else if value.eq_ignore_ascii_case("Manage") {
                    Ok(UserItemShareLevel::Manage)
                } else if value.eq_ignore_ascii_case("ManageDelete") {
                    Ok(UserItemShareLevel::ManageDelete)
                } else {
                    Err(E::unknown_variant(
                        value,
                        &["None", "Read", "Write", "Manage", "ManageDelete"],
                    ))
                }
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                match value {
                    0 => Ok(UserItemShareLevel::None),
                    1 => Ok(UserItemShareLevel::Read),
                    2 => Ok(UserItemShareLevel::Write),
                    3 => Ok(UserItemShareLevel::Manage),
                    4 => Ok(UserItemShareLevel::ManageDelete),
                    _ => Err(E::invalid_value(de::Unexpected::Signed(value), &self)),
                }
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                i64::try_from(value)
                    .map_err(|_| E::invalid_value(de::Unexpected::Unsigned(value), &self))
                    .and_then(|value| self.visit_i64(value))
            }
        }

        deserializer.deserialize_any(ShareLevelVisitor)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct IntroDebugInfo {
    id: i64,
    path: String,
    start: i64,
    end: i64,
}

// Emby's global item-sharing table has no equivalent in the Rust persistence
// layer. Reject a well-formed mutation explicitly instead of returning success
// without storing the requested access policy.
async fn update_item_access(
    request: Result<Json<CaseInsensitiveUpdateUserItemAccess>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    let Json(CaseInsensitiveUpdateUserItemAccess(request)) =
        request.map_err(|_| StatusCode::BAD_REQUEST)?;
    drop((request.item_ids, request.user_ids, request.item_access));
    Err(StatusCode::NOT_IMPLEMENTED)
}

// IntroDebugInfo belongs to Emby's proprietary intro-debug persistence, not
// Jellyfin's item-scoped media-segment API. An empty list truthfully reports
// that this server has no Emby debug records and remains decodable by clients.
async fn intro_debug_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<IntroDebugInfo>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(intro_debug_records()))
}

fn intro_debug_records() -> Vec<IntroDebugInfo> {
    Vec::new()
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
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
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

    #[tokio::test]
    async fn false_dynamic_item_routes_have_explicit_contracts() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));

        let intro_json = serde_json::to_value(intro_debug_records()).unwrap();
        assert_eq!(intro_json, serde_json::json!([]));

        for path in ["/Items/Intros", "/items/intros"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }

        for path in ["/Items/Access", "/items/access"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"iTeMiDs":[],"USERIDS":[],"itemACCESS":"manageDelete"}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
        }

        let response = app
            .oneshot(
                Request::post("/Items/Access")
                    .header("content-type", "application/json")
                    .body(Body::from("[]"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
