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
pub(crate) struct StudiosQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    #[serde(rename = "limit", alias = "Limit")]
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
    #[serde(default = "default_total_record_count")]
    #[serde(rename = "enableTotalRecordCount", alias = "EnableTotalRecordCount")]
    enable_total_record_count: bool,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<StudiosQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let enable_total_record_count = query.enable_total_record_count;
    let page = state
        .studios
        .list(
            &authenticated.user,
            target_user_id,
            ItemValueQuery {
                parent_id: query.parent_id,
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
                enable_total_record_count: Some(enable_total_record_count),
                ..ItemValueQuery::default()
            },
        )
        .await?;
    let items = page
        .studios
        .iter()
        .map(|studio| user_library::studio_to_dto(studio, state.server_id()))
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
    Path(name): Path<String>,
    Query(query): Query<StudiosQuery>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let studio = state
        .studios
        .get(&authenticated.user, target_user_id, &name)
        .await?;
    Ok(Json(user_library::studio_to_dto(
        &studio,
        state.server_id(),
    )))
}

const fn default_total_record_count() -> bool {
    true
}
