//! Provider-backed discovery for members of one Emby BoxSet.
//!
//! These legacy routes do not return persisted `BaseItemDto` values. Emby
//! asks the first enabled BoxSet metadata provider for its complete member
//! list and returns `RemoteSearchResult` values, optionally comparing their
//! provider identifiers with the collection's current linked children.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, State, rejection::PathRejection},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use jellyfin_model::RemoteSearchResult;
use serde::Serialize;
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Collections/{collection_id}/ProviderItems",
            get(provider_items),
        )
        .route(
            "/collections/{collection_id}/provideritems",
            get(provider_items),
        )
        .route("/Collections/{collection_id}/Missing", get(missing_items))
        .route("/collections/{collection_id}/missing", get(missing_items))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ProviderItemsQuery {
    user_id: Option<Uuid>,
    start_index: i32,
    limit: Option<i32>,
    is_missing: Option<bool>,
    is_unaired: Option<bool>,
    include_unaired: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteSearchQueryResult {
    items: Vec<RemoteSearchResult>,
    total_record_count: i32,
}

async fn provider_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<RemoteSearchQueryResult>, Response> {
    let Path(collection_id) = path.map_err(|error| error.into_response())?;
    let query = parse_query(uri.query()).map_err(|()| bad_request())?;
    let items = state
        .emby_collection_provider_items_for_request(
            &headers,
            &uri,
            collection_id,
            query.user_id,
            query.is_missing,
            query.is_unaired,
        )
        .await?;
    Ok(Json(page(items, query.start_index, query.limit)))
}

async fn missing_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<RemoteSearchQueryResult>, Response> {
    let Path(collection_id) = path.map_err(|error| error.into_response())?;
    let query = parse_query(uri.query()).map_err(|()| bad_request())?;
    let items = state
        .emby_collection_provider_items_for_request(
            &headers,
            &uri,
            collection_id,
            query.user_id,
            Some(true),
            (!query.include_unaired).then_some(false),
        )
        .await?;
    Ok(Json(page(items, query.start_index, query.limit)))
}

fn parse_query(raw_query: Option<&str>) -> Result<ProviderItemsQuery, ()> {
    let mut query = ProviderItemsQuery::default();
    for (name, value) in form_urlencoded::parse(raw_query.unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("UserId") {
            query.user_id = if value.trim().is_empty() {
                None
            } else {
                Some(Uuid::parse_str(value.trim()).map_err(|_| ())?)
            };
        } else if name.eq_ignore_ascii_case("StartIndex") {
            query.start_index = value.parse().map_err(|_| ())?;
        } else if name.eq_ignore_ascii_case("Limit") {
            query.limit = if value.trim().is_empty() {
                None
            } else {
                Some(value.parse().map_err(|_| ())?)
            };
        } else if name.eq_ignore_ascii_case("IsMissing") {
            query.is_missing = parse_optional_bool(&value)?;
        } else if name.eq_ignore_ascii_case("IsUnaired") {
            query.is_unaired = parse_optional_bool(&value)?;
        } else if name.eq_ignore_ascii_case("IncludeUnaired") {
            query.include_unaired = parse_bool(&value)?;
        }
    }
    Ok(query)
}

fn parse_optional_bool(value: &str) -> Result<Option<bool>, ()> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        parse_bool(value).map(Some)
    }
}

fn parse_bool(value: &str) -> Result<bool, ()> {
    if value.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if value.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(())
    }
}

fn page(
    items: Vec<RemoteSearchResult>,
    start_index: i32,
    limit: Option<i32>,
) -> RemoteSearchQueryResult {
    let total_record_count = i32::try_from(items.len()).unwrap_or(i32::MAX);
    let skip = usize::try_from(start_index).unwrap_or_default();
    let take = limit
        .map(|limit| usize::try_from(limit).unwrap_or_default())
        .unwrap_or(usize::MAX);
    RemoteSearchQueryResult {
        items: items.into_iter().skip(skip).take(take).collect(),
        total_record_count,
    }
}

fn bad_request() -> Response {
    axum::http::StatusCode::BAD_REQUEST.into_response()
}

use axum::response::IntoResponse as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_binding_is_case_insensitive_last_wins_and_signed() {
        let user_id = Uuid::new_v4();
        let query = parse_query(Some(&format!(
            "USERID={user_id}&startindex=-2&STARTINDEX=-1&limit=-3&isMissing=false&\
             ISMISSING=TRUE&isunaired=FaLsE&includeunaired=TRUE&Unknown=ignored"
        )))
        .unwrap();
        assert_eq!(query.user_id, Some(user_id));
        assert_eq!(query.start_index, -1);
        assert_eq!(query.limit, Some(-3));
        assert_eq!(query.is_missing, Some(true));
        assert_eq!(query.is_unaired, Some(false));
        assert!(query.include_unaired);
    }

    #[test]
    fn signed_paging_matches_enumerable_skip_and_take() {
        let items = (0..3)
            .map(|index| RemoteSearchResult {
                name: Some(index.to_string()),
                ..RemoteSearchResult::default()
            })
            .collect::<Vec<_>>();
        let negative_start = page(items.clone(), -1, Some(2));
        assert_eq!(negative_start.total_record_count, 3);
        assert_eq!(negative_start.items.len(), 2);
        assert!(page(items.clone(), 1, Some(0)).items.is_empty());
        assert!(page(items, 1, Some(-1)).items.is_empty());
    }

    #[test]
    fn invalid_bound_values_fail() {
        assert!(parse_query(Some("StartIndex=2147483648")).is_err());
        assert!(parse_query(Some("Limit=-2147483649")).is_err());
        assert!(parse_query(Some("IsMissing=1")).is_err());
        assert!(parse_query(Some("UserId=not-a-guid")).is_err());
    }
}
