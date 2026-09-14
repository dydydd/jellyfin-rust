use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::entities::api_key;
use jellyfin_model::{AuthenticationInfo, QueryResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CreateKeyQuery {
    #[serde(alias = "App")]
    app: Option<String>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<QueryResult<AuthenticationInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let query = is_emby_request(&uri)
        .then(|| EmbyKeysQuery::parse(&uri))
        .transpose()?;
    let keys = state.api_keys.list().await?;
    let items = keys
        .into_iter()
        .map(api_key_to_authentication_info)
        .collect::<Vec<_>>();
    Ok(Json(match query {
        Some(query) => query.page(items)?,
        None => QueryResult::from_items(items).map_err(|_| ApiError::Internal)?,
    }))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct EmbyKeysQuery {
    start_index: i32,
    limit: Option<i32>,
}

impl EmbyKeysQuery {
    fn parse(uri: &axum::http::Uri) -> Result<Self, ApiError> {
        let mut start_index = None;
        let mut limit = None;
        for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
            if name.eq_ignore_ascii_case("StartIndex") {
                start_index = Some(value.into_owned());
            } else if name.eq_ignore_ascii_case("Limit") {
                limit = Some(value.into_owned());
            }
        }

        Ok(Self {
            start_index: start_index
                .as_deref()
                .unwrap_or("0")
                .parse()
                .map_err(|_| ApiError::InvalidRequest)?,
            limit: match limit.as_deref() {
                None | Some("") => None,
                Some(limit) => Some(limit.parse().map_err(|_| ApiError::InvalidRequest)?),
            },
        })
    }

    fn page<T>(self, items: Vec<T>) -> Result<QueryResult<T>, ApiError> {
        let total_record_count = i32::try_from(items.len()).map_err(|_| ApiError::Internal)?;
        // Emby's enumerable paging skips nothing for a negative offset and
        // returns no records for a non-positive requested limit.
        let skip = usize::try_from(self.start_index).unwrap_or_default();
        let take = self
            .limit
            .map(|limit| usize::try_from(limit).unwrap_or_default())
            .unwrap_or(usize::MAX);
        Ok(QueryResult {
            items: items.into_iter().skip(skip).take(take).collect(),
            total_record_count,
            start_index: self.start_index,
        })
    }
}

fn is_emby_request(uri: &axum::http::Uri) -> bool {
    uri.path()
        .split('/')
        .nth(1)
        .is_some_and(|segment| segment.eq_ignore_ascii_case("emby"))
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<CreateKeyQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let app = query
        .app
        .as_deref()
        .map(str::trim)
        .filter(|app| !app.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    state.api_keys.create(app).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn revoke(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    state.api_keys.revoke(&key).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}

fn api_key_to_authentication_info(key: api_key::Model) -> AuthenticationInfo {
    AuthenticationInfo {
        id: key.id,
        access_token: key.access_token,
        device_id: None,
        app_name: key.name,
        app_version: None,
        device_name: None,
        user_id: Uuid::nil(),
        is_active: true,
        date_created: key.date_created,
        date_revoked: None,
        date_last_activity: key.date_last_activity,
        user_name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emby_query_is_case_insensitive_last_wins_and_signed() {
        let uri = "/emby/Auth/Keys?StartIndex=invalid&sTaRtInDeX=-2147483648&Limit=invalid&lImIt=2147483647&Unknown=ignored"
            .parse()
            .unwrap();
        assert_eq!(
            EmbyKeysQuery::parse(&uri).unwrap(),
            EmbyKeysQuery {
                start_index: i32::MIN,
                limit: Some(i32::MAX),
            }
        );
    }

    #[test]
    fn emby_query_rejects_values_outside_int32() {
        for query in [
            "StartIndex=2147483648",
            "StartIndex=-2147483649",
            "Limit=2147483648",
            "Limit=-2147483649",
            "Limit=invalid",
        ] {
            let uri = format!("/emby/Auth/Keys?{query}").parse().unwrap();
            assert!(EmbyKeysQuery::parse(&uri).is_err(), "{query}");
        }
    }

    #[test]
    fn emby_page_preserves_signed_start_and_unpaged_count() {
        let result = EmbyKeysQuery {
            start_index: -1,
            limit: Some(2),
        }
        .page(vec![1, 2, 3])
        .unwrap();
        assert_eq!(result.items, [1, 2]);
        assert_eq!(result.total_record_count, 3);
        assert_eq!(result.start_index, -1);

        let result = EmbyKeysQuery {
            start_index: 1,
            limit: Some(-1),
        }
        .page(vec![1, 2, 3])
        .unwrap();
        assert!(result.items.is_empty());
        assert_eq!(result.total_record_count, 3);
    }
}
