use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use jellyfin_controller::YearItem;
use jellyfin_data::BaseItemQuery;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct YearsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(default = "default_recursive")]
    recursive: bool,
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
    #[serde(
        default,
        rename = "mediaTypes",
        alias = "MediaTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
    #[serde(
        default,
        rename = "sortOrder",
        alias = "SortOrder",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_order: Vec<String>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<YearsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let descending = descending(&query.sort_order)?;
    let page = state
        .years
        .list(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: query.parent_id,
                recursive: query.recursive,
                include_item_types: query.include_item_types,
                exclude_item_types: query.exclude_item_types,
                media_types: query.media_types,
                start_index: query.start_index,
                limit: query.limit,
                ..BaseItemQuery::default()
            },
            descending,
        )
        .await?;
    Ok(Json(user_library::BaseItemQueryResult {
        items: page
            .years
            .iter()
            .map(|year| user_library::year_to_dto(year, state.server_id()))
            .collect(),
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
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
        YearItem::Virtual(year) => user_library::year_to_dto(&year, state.server_id()),
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

const fn default_recursive() -> bool {
    true
}
