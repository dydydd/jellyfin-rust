use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use jellyfin_controller::YearItem;
use jellyfin_data::{BaseItemQuery, ProductionYearOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct YearsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(rename = "limit", alias = "Limit")]
    limit: Option<i32>,
    #[serde(rename = "parentId", alias = "ParentId", alias = "parentid")]
    parent_id: Option<Uuid>,
    #[serde(
        default = "default_recursive",
        rename = "recursive",
        alias = "Recursive"
    )]
    recursive: bool,
    #[serde(
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        alias = "includeitemtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeItemTypes",
        alias = "ExcludeItemTypes",
        alias = "excludeitemtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_types: Vec<String>,
    #[serde(
        default,
        rename = "mediaTypes",
        alias = "MediaTypes",
        alias = "mediatypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
    #[serde(
        default,
        rename = "sortBy",
        alias = "SortBy",
        alias = "sortby",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_by: Vec<String>,
    #[serde(
        default,
        rename = "sortOrder",
        alias = "SortOrder",
        alias = "sortorder",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_order: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct YearsResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: usize,
    start_index: i32,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<YearsQuery>,
) -> Result<Json<YearsResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let order = production_year_order(&query.sort_by, &query.sort_order)?;
    let requested_start_index = query.start_index.unwrap_or_default();
    let mut item_query = BaseItemQuery {
        parent_id: query.parent_id,
        recursive: query.recursive,
        include_item_types: query.include_item_types,
        exclude_item_types: query.exclude_item_types,
        media_types: query.media_types,
        // LINQ Skip treats negative counts as zero, while the response echoes
        // the original signed StartIndex.
        start_index: u64::try_from(requested_start_index).unwrap_or_default(),
        // LINQ Take returns no items for a non-positive count.
        limit: query
            .limit
            .map(|limit| u64::try_from(limit).unwrap_or_default()),
        ..BaseItemQuery::default()
    };
    if authenticated.user.id != target_user_id && !authenticated.user.is_administrator {
        return Err(ApiError::Forbidden);
    }
    state
        .user_library
        .apply_user_policy(&mut item_query, target_user_id)
        .await?;
    if let Some(parent_id) = item_query.parent_id {
        let parent = state
            .base_items
            .get(parent_id)
            .await?
            .ok_or(ApiError::InvalidRequest)?;
        if !parent.is_folder {
            item_query.parent_id = None;
            item_query.recursive = false;
            item_query.ids = vec![parent.id];
        }
    }
    let page = state
        .years
        .list(&authenticated.user, target_user_id, item_query, order)
        .await?;
    Ok(Json(YearsResult {
        items: page
            .years
            .into_iter()
            .map(|year| user_library::year_to_dto(year, state.server_id()))
            .collect(),
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: requested_start_index,
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(year): Path<i32>,
    Query(query): Query<YearsQuery>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let year = state
        .years
        .get(&authenticated.user, target_user_id, year)
        .await?;
    Ok(Json(match year {
        YearItem::Persisted(item) => user_library::item_to_dto(item, state.server_id()),
        YearItem::Virtual(year) => user_library::year_to_dto(year, state.server_id()),
    }))
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

fn production_year_order(
    sort_by: &[String],
    sort_order: &[String],
) -> Result<ProductionYearOrder, ApiError> {
    let Some(order) = sort_by.first() else {
        return production_year_direction(sort_order);
    };

    if order.eq_ignore_ascii_case("Default")
        || order.eq_ignore_ascii_case("ProductionYear")
        || order.eq_ignore_ascii_case("SortName")
        || order.eq_ignore_ascii_case("Name")
    {
        production_year_direction(sort_order)
    } else if order.eq_ignore_ascii_case("Random") {
        Ok(ProductionYearOrder::Random)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

fn production_year_direction(sort_order: &[String]) -> Result<ProductionYearOrder, ApiError> {
    if descending(sort_order)? {
        Ok(ProductionYearOrder::Descending)
    } else {
        Ok(ProductionYearOrder::Ascending)
    }
}

const fn default_recursive() -> bool {
    true
}
