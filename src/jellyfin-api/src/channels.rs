use std::{collections::HashSet, str::FromStr, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_controller::library::{InternalItemsQuery, ItemFilter};
use jellyfin_data::{BaseItemOrder, BaseItemQuery, BaseItemRepository, entities::base_item};
use jellyfin_model::UserPolicy;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, items, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChannelsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(default, rename = "limit", alias = "Limit")]
    limit: Option<i32>,
    #[serde(
        rename = "supportsLatestItems",
        alias = "SupportsLatestItems",
        alias = "supportslatestitems"
    )]
    supports_latest_items: Option<bool>,
    #[serde(
        rename = "supportsMediaDeletion",
        alias = "SupportsMediaDeletion",
        alias = "supportsmediadeletion"
    )]
    supports_media_deletion: Option<bool>,
    #[serde(rename = "isFavorite", alias = "IsFavorite", alias = "isfavorite")]
    is_favorite: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChannelItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "folderId", alias = "FolderId", alias = "folderid")]
    folder_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(default, rename = "limit", alias = "Limit")]
    limit: Option<i32>,
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
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    filters: Vec<ChannelItemFilter>,
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
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(default, rename = "limit", alias = "Limit")]
    limit: Option<i32>,
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    filters: Vec<ChannelItemFilter>,
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
        alias = "channelids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    channel_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(clippy::struct_excessive_bools)]
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

#[derive(Debug, Clone, Copy)]
struct ChannelItemFilter(ItemFilter);

impl FromStr for ChannelItemFilter {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let filter = match value.trim().to_ascii_lowercase().as_str() {
            "isfolder" | "1" => ItemFilter::IsFolder,
            "isnotfolder" | "2" => ItemFilter::IsNotFolder,
            "isunplayed" | "3" => ItemFilter::IsUnplayed,
            "isplayed" | "4" => ItemFilter::IsPlayed,
            "isfavorite" | "5" => ItemFilter::IsFavorite,
            "isresumable" | "7" => ItemFilter::IsResumable,
            "likes" | "8" => ItemFilter::Likes,
            "dislikes" | "9" => ItemFilter::Dislikes,
            "isfavoriteorlikes" | "10" => ItemFilter::IsFavoriteOrLikes,
            _ => return Err(()),
        };
        Ok(Self(filter))
    }
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ChannelsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let requested_start_index = query.start_index.unwrap_or_default();
    // Persisted Rust channels currently have no backing provider capability
    // registry. This agrees with their exposed ChannelFeatures (all provider
    // capabilities are false), so a positive provider-capability filter has
    // no candidates while a negative filter retains every visible channel.
    let mut ids = if query.supports_latest_items == Some(true)
        || query.supports_media_deletion == Some(true)
    {
        vec![Uuid::nil()]
    } else {
        Vec::new()
    };
    let target_user = if target_user_id == authenticated.user.id {
        authenticated.user.clone()
    } else {
        state.users.get(target_user_id).await?
    };
    let policy: UserPolicy =
        serde_json::from_value(target_user.policy).map_err(|_| ApiError::Internal)?;
    let mut exclude_ids = Vec::new();
    if ids.is_empty() {
        if let Some(blocked) = policy
            .blocked_channels
            .filter(|blocked| !blocked.is_empty())
        {
            exclude_ids = blocked;
        } else if !policy.enable_all_channels {
            ids = if policy.enabled_channels.is_empty() {
                vec![Uuid::nil()]
            } else {
                policy.enabled_channels
            };
        }
    }
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                ids,
                exclude_ids,
                recursive: true,
                include_item_types: vec!["Channel".to_owned()],
                is_favorite: query.is_favorite,
                order: BaseItemOrder::SortName,
                start_index: u64::try_from(requested_start_index).unwrap_or_default(),
                // ChannelManager's in-memory GetRange path treats every non-positive
                // limit as the unbounded remainder, unlike InternalItemsQuery paging.
                limit: query
                    .limit
                    .filter(|limit| *limit > 0)
                    .map(|limit| u64::try_from(limit).unwrap_or_default()),
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    if requested_start_index < 0
        || u64::try_from(requested_start_index).unwrap_or_default() > page.total_record_count
    {
        // The official ChannelManager pages its in-memory list with List.GetRange,
        // which rejects negative and past-end indexes. Surface that server-error
        // behavior explicitly after the policy-aware query rather than panicking.
        return Err(ApiError::Internal);
    }
    let items = page
        .items
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    let mut result = user_library::BaseItemQueryResult {
        total_record_count: user_library::checked_int32(page.total_record_count)?,
        start_index: requested_start_index,
        items,
    };
    user_library::omit_incompatible_emby_relations(&uri, &mut result.items);
    Ok(Json(result))
}

pub(crate) async fn all_features(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<ChannelFeaturesDto>>, ApiError> {
    authentication::authenticated_session(&state, &headers).await?;
    let page = state
        .base_items
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
    let channel = state
        .base_items
        .get(channel_id)
        .await?
        .filter(|item| item.item_type == "Channel")
        .ok_or(ApiError::NotFound)?;
    Ok(Json(channel_features_dto(channel)))
}

pub(crate) async fn channel_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(channel_id): Path<Uuid>,
    Query(query): Query<ChannelItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let requested_start_index = query.start_index.unwrap_or_default();
    let repository = &state.base_items;
    repository
        .get(channel_id)
        .await?
        .filter(|item| item.item_type == "Channel")
        .ok_or(ApiError::NotFound)?;

    let parent_id = query.folder_id.unwrap_or(channel_id);
    if parent_id != channel_id {
        let folder = repository.get(parent_id).await?.ok_or(ApiError::NotFound)?;
        if !folder.is_folder || !is_descendant_of_channel(repository, folder.id, channel_id).await?
        {
            return Err(ApiError::NotFound);
        }
    }

    let filters = item_filter_query(&query.filters)?;
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: Some(parent_id),
                recursive: false,
                is_folder: filters.is_folder,
                is_favorite: filters.is_favorite,
                is_liked: filters.is_liked,
                is_favorite_or_liked: filters.is_favorite_or_liked,
                is_played: filters.is_played,
                is_resumable: filters.is_resumable,
                order: items::item_order(&query.sort_by, &query.sort_order),
                // InternalItemsQuery skips only positive offsets and SQLite treats
                // a negative LIMIT as unlimited. Keep PostgreSQL inputs nonnegative.
                start_index: u64::try_from(requested_start_index).unwrap_or_default(),
                limit: query
                    .limit
                    .filter(|limit| *limit >= 0)
                    .map(|limit| u64::try_from(limit).unwrap_or_default()),
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let mut result = items::page_to_dto(state.as_ref(), page, query.fields, target_user_id).await?;
    result.start_index = requested_start_index;
    user_library::omit_incompatible_emby_relations(&uri, &mut result.items);
    Ok(Json(result))
}

pub(crate) async fn latest_channel_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<LatestChannelItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let requested_start_index = query.start_index.unwrap_or_default();
    let repository = &state.base_items;
    let item_ids = channel_descendant_item_ids(repository, &query.channel_ids).await?;
    let ids = if item_ids.is_empty() {
        vec![Uuid::nil()]
    } else {
        item_ids
    };

    let filters = item_filter_query(&query.filters)?;
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                ids,
                exclude_item_types: vec!["Folder".to_owned()],
                // ChannelManager forces latest-media queries to non-folders
                // after applying the controller filters.
                is_folder: Some(false),
                is_favorite: filters.is_favorite,
                is_liked: filters.is_liked,
                is_favorite_or_liked: filters.is_favorite_or_liked,
                is_played: filters.is_played,
                is_resumable: filters.is_resumable,
                is_virtual_item: Some(false),
                order: BaseItemOrder::DateCreatedDescending,
                start_index: u64::try_from(requested_start_index).unwrap_or_default(),
                limit: query
                    .limit
                    .filter(|limit| *limit >= 0)
                    .map(|limit| u64::try_from(limit).unwrap_or_default()),
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let mut result = items::page_to_dto(state.as_ref(), page, query.fields, target_user_id).await?;
    result.start_index = requested_start_index;
    user_library::omit_incompatible_emby_relations(&uri, &mut result.items);
    Ok(Json(result))
}

fn item_filter_query(filters: &[ChannelItemFilter]) -> Result<InternalItemsQuery, ApiError> {
    let filters = filters.iter().map(|filter| filter.0).collect::<Vec<_>>();
    let mut query = InternalItemsQuery::default();
    query
        .apply_filters(&filters)
        .map_err(|_| ApiError::InvalidRequest)?;
    Ok(query)
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
    let channel_ids = if requested_channel_ids.is_empty() {
        repository
            .query(&BaseItemQuery {
                include_item_types: vec!["Channel".to_owned()],
                order: BaseItemOrder::SortName,
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            })
            .await?
            .items
            .into_iter()
            .map(|channel| channel.id)
            .collect::<Vec<_>>()
    } else {
        let requested = requested_channel_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let channels = repository
            .query(&BaseItemQuery {
                ids: requested.iter().copied().collect(),
                include_item_types: vec!["Channel".to_owned()],
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            })
            .await?
            .items;
        if channels.len() != requested.len() {
            return Err(ApiError::NotFound);
        }
        channels.into_iter().map(|channel| channel.id).collect()
    };

    Ok(repository
        .query(&BaseItemQuery {
            parent_ids: channel_ids,
            recursive: true,
            enable_total_record_count: Some(false),
            ..BaseItemQuery::default()
        })
        .await?
        .items
        .into_iter()
        .map(|item| item.id)
        .collect())
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
