use std::{collections::HashMap, str::FromStr, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
};
use axum_extra::extract::{Query, QueryRejection};
use jellyfin_controller::{UserViewGroupingOption, UserViewItem, VirtualFolder};
use jellyfin_data::BaseItemQuery;
use jellyfin_model::CollectionType;
use jellyfin_server_implementations::DtoImageOptions;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    user_library::{BaseItemDto, BaseItemQueryResult, attach_dto_image_projection},
};

const USER_VIEW_DISPLAY_PREFERENCES_ID: &str = "cb46bc72e78d95cc6cd072de3a65b93a";

const COLLECTION_TYPE_QUERY_VALUES: &[(i32, &str)] = &[
    (0, "unknown"),
    (1, "movies"),
    (2, "tvshows"),
    (3, "music"),
    (4, "musicvideos"),
    (5, "trailers"),
    (6, "homevideos"),
    (7, "boxsets"),
    (8, "books"),
    (9, "photos"),
    (10, "livetv"),
    (11, "playlists"),
    (12, "folders"),
    (101, "tvshowseries"),
    (102, "tvgenres"),
    (103, "tvgenre"),
    (104, "tvlatest"),
    (105, "tvnextup"),
    (106, "tvresume"),
    (107, "tvfavoriteseries"),
    (108, "tvfavoriteepisodes"),
    (109, "movielatest"),
    (110, "movieresume"),
    (111, "moviemovies"),
    (112, "moviecollection"),
    (113, "moviefavorites"),
    (114, "moviegenres"),
    (115, "moviegenre"),
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct CollectionTypeQuery(&'static str);

impl CollectionTypeQuery {
    pub(crate) const fn as_str(self) -> &'static str {
        self.0
    }

    pub(crate) fn as_collection_type(self) -> Option<CollectionType> {
        self.0.parse().ok()
    }
}

impl FromStr for CollectionTypeQuery {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let parsed = value
            .parse::<i32>()
            .ok()
            .and_then(|number| {
                COLLECTION_TYPE_QUERY_VALUES
                    .iter()
                    .find(|(candidate, _)| *candidate == number)
            })
            .or_else(|| {
                COLLECTION_TYPE_QUERY_VALUES
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(value))
            })
            .map(|(_, name)| *name)
            .ok_or(())?;
        Ok(Self(parsed))
    }
}

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
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    preset_views: Vec<CollectionTypeQuery>,
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
    query: Result<Query<UserViewsQuery>, QueryRejection>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let target_user_id = identity.target_user_id(query.user_id)?;
    let mut result = user_views_for(state, target_user_id, query).await?;
    crate::user_library::omit_incompatible_emby_relations(&uri, &mut result.0.items);
    Ok(result)
}

pub(crate) async fn get_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    query: Result<Query<UserViewsQuery>, QueryRejection>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let target_user_id = identity.target_user_id(Some(user_id))?;
    let mut result = user_views_for(state, target_user_id, query).await?;
    crate::user_library::omit_incompatible_emby_relations(&uri, &mut result.0.items);
    Ok(result)
}

pub(crate) async fn grouping_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<UserViewsQuery>, QueryRejection>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let target_user_id = identity.target_user_id(query.user_id)?;
    grouping_options_for(state, target_user_id).await
}

pub(crate) async fn grouping_options_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let target_user_id = identity.target_user_id(Some(user_id))?;
    grouping_options_for(state, target_user_id).await
}

async fn user_views_for(
    state: Arc<AppState>,
    target_user_id: Uuid,
    query: UserViewsQuery,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    // `UserViewQuery` initializes this to true in the official server, so an
    // omitted query value includes policy-visible external Channel views.
    let include_external_content = query.include_external_content.unwrap_or(true);
    let preset_views = query
        .preset_views
        .into_iter()
        .map(|preset| preset.as_str().to_owned())
        .collect::<Vec<_>>();
    let views = state
        .user_views
        .list(
            target_user_id,
            &preset_views,
            query.include_hidden,
            include_external_content,
        )
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
    for mut view in views {
        let view_id = view.id;
        let item_type = view.item_type.clone();
        let source_item = view.source_item.take();
        let child_count = view
            .content_parent_ids
            .iter()
            .try_fold(0_i32, |total, parent_id| {
                let count = crate::user_library::checked_int32(
                    child_counts.get(parent_id).copied().unwrap_or_default(),
                )?;
                total.checked_add(count).ok_or(ApiError::Internal)
            })?;
        let mut dto = source_item.map_or_else(
            || user_view_to_dto(view, state.server_id()),
            |item| crate::user_library::item_to_dto(item, state.server_id()),
        );
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
        total_record_count: crate::user_library::checked_int32(items.len())?,
        start_index: 0,
        items,
    }))
}

async fn grouping_options_for(
    state: Arc<AppState>,
    target_user_id: Uuid,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
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
        collection_type: base_item_collection_type(folder.collection_type),
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
        // `DtoOptions()` enables images for user views. Keep the empty map on
        // the wire as the official server does; Afuse iterates it directly.
        image_tags: Some(HashMap::new()),
        backdrop_image_tags: Some(Vec::new()),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        parent_logo_item_id: None,
        parent_logo_image_tag: None,
        parent_thumb_item_id: None,
        parent_thumb_image_tag: None,
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
        source_item: _,
    } = item;
    let collection_type = base_item_collection_type(collection_type);
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
        // See `view_to_dto`: image projection is enabled by default here.
        image_tags: Some(HashMap::new()),
        backdrop_image_tags: Some(Vec::new()),
        parent_primary_image_item_id: None,
        parent_primary_image_tag: None,
        parent_logo_item_id: None,
        parent_logo_image_tag: None,
        parent_thumb_item_id: None,
        parent_thumb_image_tag: None,
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

fn base_item_collection_type(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .and_then(|value| value.parse::<CollectionType>().ok())
        .map(|value| value.as_str().to_owned())
}

pub(crate) fn bool_option(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.as_object()?.get(*key)?.as_bool())
}
