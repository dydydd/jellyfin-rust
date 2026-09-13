//! Legacy Emby item metadata endpoints removed from current Jellyfin.
//!
//! The checked-out Jellyfin server has no critic-review store.  Its trickplay
//! implementation stores composite JPEG sprite tiles, while Emby's
//! `ThumbnailSetInfo` describes individually addressable thumbnail image tags.
//! Preserve the old wire surface without inventing reviews or claiming that a
//! sprite tile is an individual thumbnail.

use std::{convert::Infallible, fmt, sync::Arc};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/{item_id}/CriticReviews", get(critic_reviews))
        .route("/items/{item_id}/criticreviews", get(critic_reviews))
        .route("/Items/{item_id}/ThumbnailSet", get(thumbnail_set))
        .route("/items/{item_id}/thumbnailset", get(thumbnail_set))
}

#[derive(Debug, Default)]
struct CriticReviewsQuery {
    start_index: i32,
    limit: Option<i32>,
}

impl<'de> Deserialize<'de> for CriticReviewsQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct QueryVisitor;

        impl<'de> de::Visitor<'de> for QueryVisitor {
            type Value = CriticReviewsQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("signed Int32 StartIndex and Limit query values")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut query = CriticReviewsQuery::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("StartIndex") {
                        query.start_index = map.next_value()?;
                    } else if key.eq_ignore_ascii_case("Limit") {
                        query.limit = Some(map.next_value()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(query)
            }
        }

        deserializer.deserialize_map(QueryVisitor)
    }
}

#[derive(Debug)]
struct ThumbnailSetQuery {
    width: i32,
}

impl<'de> Deserialize<'de> for ThumbnailSetQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct QueryVisitor;

        impl<'de> de::Visitor<'de> for QueryVisitor {
            type Value = ThumbnailSetQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby ThumbnailSet query containing Width")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut width = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("Width") {
                        width = Some(map.next_value()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                width
                    .map(|width| ThumbnailSetQuery { width })
                    .ok_or_else(|| de::Error::missing_field("Width"))
            }
        }

        deserializer.deserialize_map(QueryVisitor)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct EmptyCriticReviewResult {
    items: Vec<Value>,
    total_record_count: i32,
}

async fn critic_reviews(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    query: Result<Query<CriticReviewsQuery>, QueryRejection>,
) -> Result<Json<EmptyCriticReviewResult>, Response> {
    // Resolve through the normal item-details route before inspecting paging.
    // This preserves hidden/missing/invalid item precedence and uses the same
    // typed, policy-aware lookup as the rest of the mobile browse surface.
    require_visible_item(&state, &headers, &uri, &item_id).await?;
    let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let _ = (query.start_index, query.limit);

    Ok(Json(EmptyCriticReviewResult {
        items: Vec::new(),
        total_record_count: 0,
    }))
}

async fn thumbnail_set(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    query: Result<Query<ThumbnailSetQuery>, QueryRejection>,
) -> Result<StatusCode, Response> {
    // Width is required by both generated SDKs. Authentication middleware runs
    // before extraction; for an authenticated request ASP.NET-style binding
    // failures remain 400 before the unavailable-resource 404.
    let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let _width = query.width;
    require_visible_item(&state, &headers, &uri, &item_id).await?;

    // Jellyfin's persisted trickplay rows point to sprite sheets and have no
    // stable per-frame ImageTag or Emby thumbnail-image route. Returning a
    // ThumbnailSetInfo would advertise unusable data, even when the requested
    // width has a real trickplay row.
    Ok(StatusCode::NOT_FOUND)
}

async fn require_visible_item(
    state: &AppState,
    headers: &HeaderMap,
    original_uri: &Uri,
    item_id: &str,
) -> Result<(), Response> {
    let item_id = Uuid::parse_str(item_id)
        .map_err(|_| StatusCode::BAD_REQUEST.into_response())?
        .hyphenated()
        .to_string();
    let mut item_uri = format!(
        "/Items?Ids={item_id}&Limit=1&EnableImages=false&EnableUserData=false&EnableTotalRecordCount=false"
    );
    if let Some(authentication_query) = authentication_query(original_uri.query()) {
        item_uri.push('&');
        item_uri.push_str(authentication_query);
    }
    let mut request = Request::get(item_uri)
        .body(Body::empty())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    *request.headers_mut() = headers.clone();
    request
        .extensions_mut()
        .insert(OriginalUri(original_uri.clone()));
    let response = jellyfin_api::unprefixed_router(state.clone())
        .oneshot(request)
        .await
        .unwrap_or_else(|error: Infallible| match error {});
    if !response.status().is_success() {
        return Err(response);
    }
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let result: Value = serde_json::from_slice(&body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    result
        .get("Items")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
        .then_some(())
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())
}

fn authentication_query(query: Option<&str>) -> Option<&str> {
    query?.split('&').find(|pair| {
        let key = pair.split_once('=').map_or(*pair, |(key, _)| key);
        key.eq_ignore_ascii_case("ApiKey") || key.eq_ignore_ascii_case("api_key")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn app() -> Router {
        routes().with_state(Arc::new(AppState::new(
            sea_orm::DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )))
    }

    #[tokio::test]
    async fn query_values_use_case_insensitive_signed_int32_binding() {
        // These requests stop at the disconnected item lookup only after the
        // query extractor has accepted the complete signed range.
        for path in [
            "/Items/00000000-0000-0000-0000-000000000001/ThumbnailSet?Width=-2147483648",
            "/items/00000000-0000-0000-0000-000000000001/thumbnailset?wIdTh=2147483647",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_ne!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }

        for path in [
            "/Items/id/ThumbnailSet",
            "/Items/id/ThumbnailSet?Width=nope",
            "/Items/id/ThumbnailSet?Width=2147483648",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
    }
}
