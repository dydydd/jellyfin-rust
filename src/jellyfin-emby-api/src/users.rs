//! Emby-only user query contracts missing from Jellyfin's public surface.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Query, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use jellyfin_model::{NameIdPair, UserDto};
use serde::{Deserialize, Serialize};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Users/Query", get(query_users))
        .route("/users/query", get(query_users))
        .route("/Users/ItemAccess", get(item_access))
        .route("/users/itemaccess", get(item_access))
        .route("/Users/Prefixes", get(prefixes))
        .route("/users/prefixes", get(prefixes))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UserQuery {
    #[serde(rename = "IsHidden", alias = "isHidden", alias = "ishidden")]
    is_hidden: Option<bool>,
    #[serde(rename = "IsDisabled", alias = "isDisabled", alias = "isdisabled")]
    is_disabled: Option<bool>,
    #[serde(rename = "StartIndex", alias = "startIndex", alias = "startindex")]
    start_index: Option<i32>,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i32>,
    #[serde(
        rename = "NameStartsWithOrGreater",
        alias = "nameStartsWithOrGreater",
        alias = "namestartswithorgreater"
    )]
    name_starts_with_or_greater: Option<String>,
    #[serde(rename = "SortOrder", alias = "sortOrder", alias = "sortorder")]
    sort_order: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct UserQueryResult {
    items: Vec<UserDto>,
    total_record_count: i32,
    start_index: i32,
}

async fn query_users(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<UserQueryResult>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(page(
        state.emby_users(query.is_hidden, query.is_disabled).await?,
        query,
    )))
}

async fn item_access(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UserQuery>,
) -> Result<Json<UserQueryResult>, Response> {
    // The protocol middleware has already enforced authentication. ItemAccess
    // intentionally exposes the same user projection as Emby's service.
    Ok(Json(page(
        state.emby_users(query.is_hidden, query.is_disabled).await?,
        query,
    )))
}

async fn prefixes(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserQuery>,
) -> Result<Json<Vec<NameIdPair>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let users = state.emby_users(query.is_hidden, query.is_disabled).await?;
    let mut prefixes = Vec::new();
    for user in users {
        let Some(name) = user.name else { continue };
        if query
            .name_starts_with_or_greater
            .as_deref()
            .is_some_and(|start| name.as_str() < start)
        {
            continue;
        }
        let Some(prefix) = name.chars().next().map(|c| c.to_uppercase().to_string()) else {
            continue;
        };
        if !prefixes.iter().any(|item: &NameIdPair| item.name == prefix) {
            prefixes.push(NameIdPair {
                name: prefix.clone(),
                id: prefix,
            });
        }
    }
    prefixes.sort_by(|a, b| a.name.cmp(&b.name));
    if query
        .sort_order
        .as_deref()
        .is_some_and(|order| order.eq_ignore_ascii_case("descending") || order == "1")
    {
        prefixes.reverse();
    }
    let start = query.start_index.unwrap_or(0);
    let offset = start.max(0) as usize;
    let end = match query.limit {
        Some(0) => offset,
        Some(limit) if limit > 0 => offset.saturating_add(limit as usize),
        _ => prefixes.len(),
    }
    .min(prefixes.len());
    let prefixes = if offset < prefixes.len() {
        prefixes[offset..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(Json(prefixes))
}

fn page(mut users: Vec<UserDto>, query: UserQuery) -> UserQueryResult {
    if let Some(prefix) = query.name_starts_with_or_greater.as_deref() {
        users.retain(|user| user.name.as_deref().is_some_and(|name| name >= prefix));
    }
    if query
        .sort_order
        .as_deref()
        .is_some_and(|order| order.eq_ignore_ascii_case("descending") || order == "1")
    {
        users.reverse();
    }
    let total = users.len().min(i32::MAX as usize) as i32;
    let start = query.start_index.unwrap_or(0);
    let offset = start.max(0) as usize;
    let limit = query.limit.unwrap_or(-1);
    let items = if limit == 0 || offset >= users.len() {
        Vec::new()
    } else {
        let end = if limit < 0 {
            users.len()
        } else {
            offset.saturating_add(limit as usize).min(users.len())
        };
        users[offset..end].to_vec()
    };
    UserQueryResult {
        items,
        total_record_count: total,
        start_index: start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_paging_matches_emby_contract() {
        let users = (0..3)
            .map(|n| UserDto {
                name: Some(format!("{n}")),
                ..Default::default()
            })
            .collect();
        let result = page(
            users,
            UserQuery {
                start_index: Some(-2),
                limit: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(result.start_index, -2);
        assert_eq!(result.total_record_count, 3);
        assert_eq!(result.items.len(), 1);
    }
}
