use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_controller::{UserViewGroupingOption, UserViewItem, VirtualFolder};
use jellyfin_data::BaseItemQuery;
use jellyfin_server_implementations::DtoImageOptions;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    user_library::{BaseItemDto, BaseItemQueryResult, attach_dto_image_projection},
};

const USER_VIEW_DISPLAY_PREFERENCES_ID: &str = "cb46bc72e78d95cc6cd072de3a65b93a";

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserViewsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "includeExternalContent",
        alias = "IncludeExternalContent",
        alias = "includeexternalcontent"
    )]
    include_external_content: Option<bool>,
    #[serde(
        default,
        rename = "presetViews",
        alias = "PresetViews",
        alias = "presetviews",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    preset_views: Vec<String>,
    #[serde(
        default,
        rename = "includeHidden",
        alias = "IncludeHidden",
        alias = "includehidden"
    )]
    include_hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SpecialViewOptionDto {
    name: String,
    id: String,
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserViewsQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    user_views_for(state, headers, &uri, query.user_id, query).await
}

pub(crate) async fn get_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<UserViewsQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    user_views_for(state, headers, &uri, Some(user_id), query).await
}

pub(crate) async fn grouping_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserViewsQuery>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    grouping_options_for(state, headers, &uri, query.user_id).await
}

pub(crate) async fn grouping_options_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    grouping_options_for(state, headers, &uri, Some(user_id)).await
}

async fn user_views_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
    query: UserViewsQuery,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let target_user_id = target_user_id(&state, &headers, uri, requested_user_id).await?;
    let _ = query.include_external_content;
    let views = state
        .user_views
        .list(target_user_id, &query.preset_views, query.include_hidden)
        .await?;
    let target_user = state.users.get(target_user_id).await?;
    let mut parent_ids = views
        .iter()
        .flat_map(|view| view.content_parent_ids.iter().copied())
        .collect::<Vec<_>>();
    parent_ids.sort_unstable();
    parent_ids.dedup();
    let child_counts = state
        .user_library
        .child_counts_by_parent(
            &target_user,
            target_user_id,
            BaseItemQuery {
                parent_ids,
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let view_ids = views.iter().map(|view| view.id).collect::<Vec<_>>();
    let mut image_projections = state
        .dto_images
        .project_many(
            &view_ids,
            DtoImageOptions {
                include_primary_image_aspect_ratio: true,
                ..DtoImageOptions::default()
            },
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    let mut items = Vec::with_capacity(views.len());
    for view in views {
        let view_id = view.id;
        let item_type = view.item_type.clone();
        let child_count = view
            .content_parent_ids
            .iter()
            .map(|parent_id| child_counts.get(parent_id).copied().unwrap_or_default())
            .sum();
        let mut dto = user_view_to_dto(view, state.server_id());
        dto.child_count = Some(child_count);
        dto.display_preferences_id = Some(if item_type.eq_ignore_ascii_case("UserView") {
            USER_VIEW_DISPLAY_PREFERENCES_ID.to_owned()
        } else {
            view_id.simple().to_string()
        });
        if let Some(projection) = image_projections.remove(&view_id) {
            attach_dto_image_projection(&mut dto, projection);
        }
        items.push(dto);
    }
    Ok(Json(BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}

async fn grouping_options_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    let target_user_id = target_user_id(&state, &headers, uri, requested_user_id).await?;
    Ok(Json(
        state
            .user_views
            .grouping_options(target_user_id)
            .await?
            .into_iter()
            .map(|option| {
                let UserViewGroupingOption { id, name } = option;
                SpecialViewOptionDto {
                    name,
                    id: id.simple().to_string(),
                }
            })
            .collect(),
    ))
}

async fn target_user_id(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.target_user_id(requested_user_id)
}

pub(crate) fn view_to_dto(folder: VirtualFolder, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(folder.name.clone()),
        server_id: server_id.to_owned(),
        id: folder.id.simple().to_string(),
        playlist_item_id: None,
        item_type: "CollectionFolder".to_owned(),
        etag: folder.id.simple().to_string(),
        date_created: None,
        sort_name: Some(folder.name),
        path: None,
        overview: None,
        media_type: None,
        collection_type: folder.collection_type,
        is_folder: true,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: None,
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn user_view_to_dto(item: UserViewItem, server_id: &str) -> BaseItemDto {
    let UserViewItem {
        id,
        name,
        collection_type,
        display_parent_id: _,
        content_parent_ids: _,
        parent_id,
        item_type,
        is_virtual_item,
    } = item;
    BaseItemDto {
        // ALLOW: Jellyfin exposes name and sort name as independent owned fields.
        name: Some(name.clone()),
        server_id: server_id.to_owned(),
        id: id.simple().to_string(),
        playlist_item_id: None,
        item_type,
        etag: id.simple().to_string(),
        date_created: None,
        sort_name: Some(name),
        path: None,
        overview: None,
        media_type: None,
        collection_type,
        is_folder: true,
        is_virtual_item,
        parent_id: parent_id.map(|id| id.simple().to_string()),
        index_number: None,
        parent_index_number: None,
        production_year: None,
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: None,
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        image_tags: HashMap::new(),
        backdrop_image_tags: Vec::new(),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        primary_image_aspect_ratio: None,
        series_primary_image_tag: None,
        parent_backdrop_image_item_id: None,
        parent_backdrop_image_tags: Vec::new(),
        media_sources: None,
        media_streams: None,
        trickplay: None,
        ..BaseItemDto::default()
    }
}

pub(crate) fn bool_option(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.as_object()?.get(*key)?.as_bool())
}
