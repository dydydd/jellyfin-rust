use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_data::{BaseItemOrder, BaseItemPage, BaseItemQuery};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    recursive: Option<bool>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    search_term: Option<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    ids: Vec<Uuid>,
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
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    get_for(state, headers, query.user_id, query).await
}

pub(crate) async fn get_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    get_for(state, headers, Some(user_id), query).await
}

pub(crate) async fn resume(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    resume_for(state, headers, query.user_id, query).await
}

pub(crate) async fn resume_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    resume_for(state, headers, Some(user_id), query).await
}

async fn get_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: ItemsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let fields = query.fields.clone();
    let page = state
        .user_library
        .query_items(&authenticated.user, target_user_id, query.try_into()?)
        .await?;
    Ok(Json(page_to_dto(state.as_ref(), page, fields).await?))
}

async fn resume_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: ItemsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let fields = query.fields.clone();
    let page = state
        .user_library
        .resume_items(&authenticated.user, target_user_id, query.try_into()?)
        .await?;
    Ok(Json(page_to_dto(state.as_ref(), page, fields).await?))
}

impl TryFrom<ItemsQuery> for BaseItemQuery {
    type Error = ApiError;

    fn try_from(query: ItemsQuery) -> Result<Self, Self::Error> {
        Ok(Self {
            ids: query.ids,
            exclude_ids: Vec::new(),
            parent_id: query.parent_id,
            recursive: query.recursive.unwrap_or(false),
            search_term: query.search_term,
            include_item_types: query.include_item_types,
            exclude_item_types: query.exclude_item_types,
            media_types: query.media_types,
            is_virtual_item: None,
            group_versions_by_presentation_key: false,
            user_id: query.user_id,
            is_resumable: None,
            order: BaseItemOrder::default(),
            start_index: query.start_index,
            limit: query.limit,
        })
    }
}

async fn page_to_dto(
    state: &AppState,
    page: BaseItemPage,
    fields: Vec<String>,
) -> Result<user_library::BaseItemQueryResult, ApiError> {
    let requested_fields = user_library::BaseItemDtoFields::from_names(&fields);
    let mut media_streams = if requested_fields.wants_media_streams() {
        let item_ids = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
        state
            .media_streams
            .get_media_streams_for_items(&item_ids)
            .await?
    } else {
        std::collections::HashMap::new()
    };

    let mut items = Vec::with_capacity(page.items.len());
    for item in page.items {
        let item_id = item.id;
        let mut dto = user_library::item_to_dto(item, state.server_id());
        if requested_fields.wants_media_streams() {
            let streams = media_streams.remove(&item_id).unwrap_or_default();
            user_library::project_item_dto_with_streams(&mut dto, requested_fields, streams);
        }
        items.push(dto);
    }

    Ok(user_library::BaseItemQueryResult {
        items,
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
    })
}
