use std::{collections::HashSet, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_data::{BaseItemOrder, BaseItemQuery, BaseItemRepository, entities::base_item};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, items, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChannelsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(rename = "supportsLatestItems", alias = "SupportsLatestItems")]
    supports_latest_items: Option<bool>,
    #[serde(rename = "supportsMediaDeletion", alias = "SupportsMediaDeletion")]
    supports_media_deletion: Option<bool>,
    #[serde(rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChannelItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "folderId", alias = "FolderId")]
    folder_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
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
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    filters: Vec<String>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct LatestChannelItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    filters: Vec<String>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(
        default,
        rename = "channelIds",
        alias = "ChannelIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    channel_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ChannelFeaturesDto {
    name: String,
    id: Uuid,
    can_search: bool,
    media_types: Vec<String>,
    content_types: Vec<String>,
    max_page_size: Option<i32>,
    auto_refresh_levels: Option<i32>,
    default_sort_fields: Vec<String>,
    supports_sort_order_toggle: bool,
    supports_latest_media: bool,
    can_filter: bool,
    supports_content_downloading: bool,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ChannelsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let _ = (
        query.supports_latest_items,
        query.supports_media_deletion,
        query.is_favorite,
    );
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                recursive: true,
                include_item_types: vec!["Channel".to_owned()],
                order: BaseItemOrder::SortName,
                start_index: query.start_index,
                limit: query.limit,
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let items = page
        .items
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
        items,
    }))
}

pub(crate) async fn all_features(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<ChannelFeaturesDto>>, ApiError> {
    authentication::authenticated_session(&state, &headers).await?;
    let page = BaseItemRepository::new(state.database.clone())
        .query(&BaseItemQuery {
            include_item_types: vec!["Channel".to_owned()],
            order: BaseItemOrder::SortName,
            enable_total_record_count: Some(false),
            ..BaseItemQuery::default()
        })
        .await?;
    Ok(Json(
        page.items.into_iter().map(channel_features_dto).collect(),
    ))
}

pub(crate) async fn features(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(channel_id): Path<Uuid>,
) -> Result<Json<ChannelFeaturesDto>, ApiError> {
    authentication::authenticated_session(&state, &headers).await?;
    let channel = BaseItemRepository::new(state.database.clone())
        .get(channel_id)
        .await?
        .filter(|item| item.item_type == "Channel")
        .ok_or(ApiError::NotFound)?;
    Ok(Json(channel_features_dto(channel)))
}

pub(crate) async fn channel_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(channel_id): Path<Uuid>,
    Query(query): Query<ChannelItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let repository = BaseItemRepository::new(state.database.clone());
    repository
        .get(channel_id)
        .await?
        .filter(|item| item.item_type == "Channel")
        .ok_or(ApiError::NotFound)?;

    let parent_id = query.folder_id.unwrap_or(channel_id);
    if parent_id != channel_id {
        let folder = repository.get(parent_id).await?.ok_or(ApiError::NotFound)?;
        if !folder.is_folder
            || !is_descendant_of_channel(&repository, folder.id, channel_id).await?
        {
            return Err(ApiError::NotFound);
        }
    }

    let _ = (query.filters, query.fields);
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: Some(parent_id),
                recursive: false,
                order: items::item_order(&query.sort_by, &query.sort_order),
                start_index: query.start_index,
                limit: query.limit,
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let items = page
        .items
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
        items,
    }))
}

pub(crate) async fn latest_channel_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LatestChannelItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let repository = BaseItemRepository::new(state.database.clone());
    let item_ids = channel_descendant_item_ids(&repository, &query.channel_ids).await?;
    let ids = if item_ids.is_empty() {
        vec![Uuid::nil()]
    } else {
        item_ids
    };

    let _ = (query.filters, query.fields);
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                ids,
                exclude_item_types: vec!["Folder".to_owned()],
                is_virtual_item: Some(false),
                order: BaseItemOrder::DateCreatedDescending,
                start_index: query.start_index,
                limit: query.limit,
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let items = page
        .items
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
        items,
    }))
}

fn channel_features_dto(channel: base_item::Model) -> ChannelFeaturesDto {
    ChannelFeaturesDto {
        name: channel.name.unwrap_or_default(),
        id: channel.id,
        can_search: false,
        media_types: Vec::new(),
        content_types: Vec::new(),
        max_page_size: None,
        auto_refresh_levels: None,
        default_sort_fields: Vec::new(),
        supports_sort_order_toggle: false,
        supports_latest_media: false,
        can_filter: true,
        supports_content_downloading: false,
    }
}

async fn channel_descendant_item_ids(
    repository: &BaseItemRepository,
    requested_channel_ids: &[Uuid],
) -> Result<Vec<Uuid>, ApiError> {
    let channels = if requested_channel_ids.is_empty() {
        repository
            .query(&BaseItemQuery {
                include_item_types: vec!["Channel".to_owned()],
                order: BaseItemOrder::SortName,
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            })
            .await?
            .items
    } else {
        let mut channels = Vec::with_capacity(requested_channel_ids.len());
        for channel_id in requested_channel_ids {
            channels.push(
                repository
                    .get(*channel_id)
                    .await?
                    .filter(|item| item.item_type == "Channel")
                    .ok_or(ApiError::NotFound)?,
            );
        }
        channels
    };

    let mut seen = HashSet::new();
    let mut item_ids = Vec::new();
    for channel in channels {
        for descendant in repository.descendants(channel.id).await? {
            if seen.insert(descendant.item.id) {
                item_ids.push(descendant.item.id);
            }
        }
    }
    Ok(item_ids)
}

async fn is_descendant_of_channel(
    repository: &BaseItemRepository,
    item_id: Uuid,
    channel_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(repository
        .ancestors(item_id)
        .await?
        .into_iter()
        .any(|entry| entry.item.id == channel_id))
}
