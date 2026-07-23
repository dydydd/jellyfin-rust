use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use jellyfin_data::ItemValueQuery;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GenresQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    search_term: Option<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeItemTypes",
        alias = "ExcludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_types: Vec<String>,
    #[serde(default, rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
    #[serde(rename = "nameStartsWithOrGreater", alias = "NameStartsWithOrGreater")]
    name_starts_with_or_greater: Option<String>,
    #[serde(rename = "nameStartsWith", alias = "NameStartsWith")]
    name_starts_with: Option<String>,
    #[serde(rename = "nameLessThan", alias = "NameLessThan")]
    name_less_than: Option<String>,
    #[serde(
        default,
        rename = "sortBy",
        alias = "SortBy",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_by: Vec<String>,
    #[serde(
        default,
        rename = "sortOrder",
        alias = "SortOrder",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_order: Vec<String>,
    #[serde(default = "default_total_record_count")]
    #[serde(rename = "enableTotalRecordCount", alias = "EnableTotalRecordCount")]
    enable_total_record_count: bool,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<GenresQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let order = crate::query::item_value_order(&query.sort_by)?;
    let descending = descending(&query.sort_order)?;
    let enable_total_record_count = query.enable_total_record_count;
    let page = state
        .genres
        .list(
            &authenticated.user,
            target_user_id,
            ItemValueQuery {
                parent_id: query.parent_id,
                recursive: true,
                search_term: query.search_term,
                include_item_types: query.include_item_types,
                exclude_item_types: query.exclude_item_types,
                is_favorite: query.is_favorite,
                user_id: Some(target_user_id),
                name_starts_with_or_greater: query.name_starts_with_or_greater,
                name_starts_with: query.name_starts_with,
                name_less_than: query.name_less_than,
                start_index: query.start_index,
                limit: query.limit,
                order,
                descending,
                enable_total_record_count: Some(enable_total_record_count),
                ..ItemValueQuery::default()
            },
        )
        .await?;
    let items = page
        .genres
        .iter()
        .map(|genre| user_library::genre_to_dto(genre, state.server_id()))
        .collect::<Vec<_>>();
    let total_record_count = if enable_total_record_count {
        usize::try_from(page.total_record_count).unwrap_or(usize::MAX)
    } else {
        items.len()
    };
    Ok(Json(user_library::BaseItemQueryResult {
        items,
        total_record_count,
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(genre_name): Path<String>,
    Query(query): Query<GenresQuery>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let genre = state
        .genres
        .get(&authenticated.user, target_user_id, &genre_name)
        .await?;
    Ok(Json(user_library::genre_to_dto(&genre, state.server_id())))
}

fn descending(sort_order: &[String]) -> Result<bool, ApiError> {
    let Some(order) = sort_order.first() else {
        return Ok(false);
    };
    if order.eq_ignore_ascii_case("Descending") {
        Ok(true)
    } else if order.eq_ignore_ascii_case("Ascending") {
        Ok(false)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

const fn default_total_record_count() -> bool {
    true
}
