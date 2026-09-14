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
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::{AppState, EmbyItemAccessMutation, EmbyLeaveSharedItemsMutation};
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
        .route("/Items/Shared/Leave", post(leave_shared_items))
        .route("/items/shared/leave", post(leave_shared_items))
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
        .route("/VideoCodecs", get(video_codecs))
        .route("/videocodecs", get(video_codecs))
        .route("/SubtitleCodecs", get(subtitle_codecs))
        .route("/subtitlecodecs", get(subtitle_codecs))
        .route("/StreamLanguages", get(stream_languages))
        .route("/streamlanguages", get(stream_languages))
        .route("/Tags", get(tags))
        .route("/tags", get(tags))
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

#[derive(Debug)]
struct LeaveSharedItems {
    item_ids: Option<Vec<String>>,
    user_id: Option<String>,
}

#[derive(Debug)]
struct CaseInsensitiveLeaveSharedItems(LeaveSharedItems);

impl<'de> Deserialize<'de> for CaseInsensitiveLeaveSharedItems {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LeaveVisitor;

        impl<'de> de::Visitor<'de> for LeaveVisitor {
            type Value = CaseInsensitiveLeaveSharedItems;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby leave shared items object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut item_ids = None;
                let mut user_id = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("ItemIds") {
                        item_ids = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("UserId") {
                        user_id = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(CaseInsensitiveLeaveSharedItems(LeaveSharedItems {
                    item_ids,
                    user_id,
                }))
            }
        }

        deserializer.deserialize_map(LeaveVisitor)
    }
}

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
                } else if let Ok(value) = value.parse::<i64>() {
                    self.visit_i64(value)
                } else {
                    Err(E::unknown_variant(
                        value,
                        &["None", "Read", "Write", "Manage", "ManageDelete"],
                    ))
                }
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                // Emby's generated .NET clients expose these underlying enum
                // values as one-based even though the normal wire form is a
                // string. Json.NET also permits their numeric representation.
                match value {
                    1 => Ok(UserItemShareLevel::None),
                    2 => Ok(UserItemShareLevel::Read),
                    3 => Ok(UserItemShareLevel::Write),
                    4 => Ok(UserItemShareLevel::Manage),
                    5 => Ok(UserItemShareLevel::ManageDelete),
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

#[allow(clippy::result_large_err)]
async fn update_item_access(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<CaseInsensitiveUpdateUserItemAccess>, JsonRejection>,
) -> Result<StatusCode, Response> {
    let mutation = request
        .ok()
        .map(
            |Json(CaseInsensitiveUpdateUserItemAccess(request))| EmbyItemAccessMutation {
                item_ids: request.item_ids,
                user_ids: request.user_ids,
                access_level: request.item_access.and_then(|access| match access {
                    UserItemShareLevel::None => None,
                    UserItemShareLevel::Read => Some(1),
                    UserItemShareLevel::Write => Some(2),
                    UserItemShareLevel::Manage => Some(3),
                    UserItemShareLevel::ManageDelete => Some(4),
                }),
            },
        );
    state
        .update_emby_item_access_for_request(&headers, &uri, mutation)
        .await
}

#[allow(clippy::result_large_err)]
async fn leave_shared_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<CaseInsensitiveLeaveSharedItems>, JsonRejection>,
) -> Result<StatusCode, Response> {
    let mutation = request
        .ok()
        .map(
            |Json(CaseInsensitiveLeaveSharedItems(request))| EmbyLeaveSharedItemsMutation {
                item_ids: request.item_ids,
                user_id: request.user_id,
            },
        );
    state
        .leave_emby_shared_items_for_request(&headers, &uri, mutation)
        .await
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
    total_record_count: i32,
    items: Vec<TagItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TagItem {
    name: String,
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct OfficialRatingItem {
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct OfficialRatingResult {
    total_record_count: i32,
    items: Vec<OfficialRatingItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FeatureInfo {
    feature_type: String,
    id: String,
    name: String,
}

async fn item_prefixes(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<NameValuePair>>, Response> {
    let (values, _) = state
        .emby_library_facet_for_request(&headers, &uri, "ItemPrefix")
        .await?;
    Ok(Json(name_value_pairs(values)))
}

async fn artist_prefixes(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<NameValuePair>>, Response> {
    let (values, _) = state
        .emby_library_facet_for_request(&headers, &uri, "ArtistPrefix")
        .await?;
    Ok(Json(name_value_pairs(values)))
}

async fn item_types(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "ItemType").await
}

async fn audio_codecs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "AudioCodec").await
}

async fn audio_layouts(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "AudioLayout").await
}

async fn video_codecs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "VideoCodec").await
}

async fn subtitle_codecs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "SubtitleCodec").await
}

async fn stream_languages(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "StreamLanguage").await
}

async fn tags(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "Tag").await
}

async fn containers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "Container").await
}

async fn extended_video_types(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<TagResult>, Response> {
    tag_facet(&state, &headers, &uri, "ExtendedVideoType").await
}

async fn official_ratings(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<OfficialRatingResult>, Response> {
    let (values, total_record_count) = state
        .emby_library_facet_for_request(&headers, &uri, "OfficialRating")
        .await?;
    Ok(Json(OfficialRatingResult {
        total_record_count: checked_total(total_record_count)?,
        items: values
            .into_iter()
            .map(|name| OfficialRatingItem { name })
            .collect(),
    }))
}

async fn features(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FeatureInfo>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(Vec::new()))
}

async fn tag_facet(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    facet: &str,
) -> Result<Json<TagResult>, Response> {
    let (values, total_record_count) = state
        .emby_library_facet_for_request(headers, uri, facet)
        .await?;
    Ok(Json(TagResult {
        total_record_count: checked_total(total_record_count)?,
        items: values
            .into_iter()
            .map(|name| TagItem {
                id: name.clone(),
                name,
            })
            .collect(),
    }))
}

fn name_value_pairs(values: Vec<String>) -> Vec<NameValuePair> {
    values
        .into_iter()
        .map(|name| NameValuePair {
            value: name.clone(),
            name,
        })
        .collect()
}

fn checked_total(total: u64) -> Result<i32, Response> {
    i32::try_from(total).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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

    #[test]
    fn discovery_dtos_match_generated_sdk_shapes() {
        let tags = TagResult {
            total_record_count: 1,
            items: vec![TagItem {
                name: "aac".to_owned(),
                id: "aac".to_owned(),
            }],
        };
        assert_eq!(
            serde_json::to_value(tags).unwrap(),
            serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{"Name": "aac", "Id": "aac"}]
            })
        );
        assert_eq!(
            serde_json::to_value(OfficialRatingResult {
                total_record_count: 1,
                items: vec![OfficialRatingItem {
                    name: "PG-13".to_owned(),
                }],
            })
            .unwrap(),
            serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{"Name": "PG-13"}]
            })
        );
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
                    Request::post(path)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"iTeMiDs":[],"USERIDS":[],"itemACCESS":"manageDelete"}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }

        for path in ["/Items/Shared/Leave", "/items/shared/leave"] {
            let response = app
                .clone()
                .oneshot(
                    Request::post(path)
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"iTeMiDs":[],"uSeRiD":null}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    #[test]
    fn item_access_body_binds_case_insensitively_and_uses_last_duplicate() {
        let parsed: CaseInsensitiveUpdateUserItemAccess = serde_json::from_str(
            r#"{"ItemIds":["first"],"itemids":["second"],"USERIDS":[],"ItemAccess":1,"itemaccess":"ManageDelete","unknown":true}"#,
        )
        .expect("case-insensitive body");
        assert_eq!(parsed.0.item_ids, Some(vec!["second".to_owned()]));
        assert_eq!(parsed.0.user_ids, Some(Vec::new()));
        assert_eq!(parsed.0.item_access, Some(UserItemShareLevel::ManageDelete));

        for value in ["None", "read", "WRITE", "Manage", "manageDelete"] {
            let body = format!(r#"{{"ItemAccess":"{value}"}}"#);
            assert!(serde_json::from_str::<CaseInsensitiveUpdateUserItemAccess>(&body).is_ok());
        }
        for value in 1..=5 {
            let body = format!(r#"{{"ItemAccess":{value}}}"#);
            assert!(serde_json::from_str::<CaseInsensitiveUpdateUserItemAccess>(&body).is_ok());
            let body = format!(r#"{{"ItemAccess":"{value}"}}"#);
            assert!(serde_json::from_str::<CaseInsensitiveUpdateUserItemAccess>(&body).is_ok());
        }
        for value in [0, 6] {
            let body = format!(r#"{{"ItemAccess":{value}}}"#);
            assert!(serde_json::from_str::<CaseInsensitiveUpdateUserItemAccess>(&body).is_err());
        }
    }

    #[test]
    fn shared_leave_body_binds_case_insensitively_and_uses_last_duplicate() {
        let parsed: CaseInsensitiveLeaveSharedItems = serde_json::from_str(
            r#"{"ItemIds":["first"],"itemids":["second"],"UserId":"first-user","USERID":"second-user","unknown":true}"#,
        )
        .expect("case-insensitive body");
        assert_eq!(parsed.0.item_ids, Some(vec!["second".to_owned()]));
        assert_eq!(parsed.0.user_id.as_deref(), Some("second-user"));

        let parsed: CaseInsensitiveLeaveSharedItems =
            serde_json::from_str(r#"{"ItemIds":null,"UserId":null}"#)
                .expect("nullable generated properties");
        assert_eq!(parsed.0.item_ids, None);
        assert_eq!(parsed.0.user_id, None);
    }
}
