use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use chrono::{DateTime, Duration, Utc};
use jellyfin_controller::UserLibraryError;
use jellyfin_data::{BaseItemOrder, BaseItemQuery, entities::base_item};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SeasonsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(rename = "isSpecialSeason", alias = "IsSpecialSeason")]
    is_special_season: Option<bool>,
    #[serde(rename = "isMissing", alias = "IsMissing")]
    is_missing: Option<bool>,
    #[serde(rename = "adjacentTo", alias = "AdjacentTo")]
    adjacent_to: Option<Uuid>,
    #[serde(rename = "enableImages", alias = "EnableImages")]
    enable_images: Option<bool>,
    #[serde(rename = "imageTypeLimit", alias = "ImageTypeLimit")]
    image_type_limit: Option<i32>,
    #[serde(
        default,
        rename = "enableImageTypes",
        alias = "EnableImageTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    enable_image_types: Vec<String>,
    #[serde(rename = "enableUserData", alias = "EnableUserData")]
    enable_user_data: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct EpisodesQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    season: Option<i32>,
    #[serde(rename = "seasonId", alias = "SeasonId")]
    season_id: Option<Uuid>,
    #[serde(rename = "isMissing", alias = "IsMissing")]
    is_missing: Option<bool>,
    #[serde(rename = "adjacentTo", alias = "AdjacentTo")]
    adjacent_to: Option<Uuid>,
    #[serde(rename = "startItemId", alias = "StartItemId")]
    start_item_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(rename = "enableImages", alias = "EnableImages")]
    enable_images: Option<bool>,
    #[serde(rename = "imageTypeLimit", alias = "ImageTypeLimit")]
    image_type_limit: Option<i32>,
    #[serde(
        default,
        rename = "enableImageTypes",
        alias = "EnableImageTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    enable_image_types: Vec<String>,
    #[serde(rename = "enableUserData", alias = "EnableUserData")]
    enable_user_data: Option<bool>,
    #[serde(rename = "sortBy", alias = "SortBy")]
    sort_by: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct NextUpQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(rename = "seriesId", alias = "SeriesId")]
    series_id: Option<Uuid>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(rename = "enableImages", alias = "EnableImages")]
    enable_images: Option<bool>,
    #[serde(rename = "imageTypeLimit", alias = "ImageTypeLimit")]
    image_type_limit: Option<i32>,
    #[serde(
        default,
        rename = "enableImageTypes",
        alias = "EnableImageTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    enable_image_types: Vec<String>,
    #[serde(rename = "enableUserData", alias = "EnableUserData")]
    enable_user_data: Option<bool>,
    #[serde(rename = "nextUpDateCutoff", alias = "NextUpDateCutoff")]
    next_up_date_cutoff: Option<String>,
    #[serde(
        default = "default_enable_total_record_count",
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount"
    )]
    enable_total_record_count: bool,
    #[serde(
        default = "default_true",
        rename = "enableResumable",
        alias = "EnableResumable"
    )]
    enable_resumable: bool,
    #[serde(default, rename = "enableRewatching", alias = "EnableRewatching")]
    enable_rewatching: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpcomingQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(rename = "enableImages", alias = "EnableImages")]
    enable_images: Option<bool>,
    #[serde(rename = "imageTypeLimit", alias = "ImageTypeLimit")]
    image_type_limit: Option<i32>,
    #[serde(
        default,
        rename = "enableImageTypes",
        alias = "EnableImageTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    enable_image_types: Vec<String>,
    #[serde(rename = "enableUserData", alias = "EnableUserData")]
    enable_user_data: Option<bool>,
}

pub(crate) async fn upcoming(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpcomingQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let fields = user_library::BaseItemDtoFields::from_names(&query.fields);
    let _ = (
        query.enable_images,
        query.image_type_limit,
        query.enable_image_types,
        query.enable_user_data,
    );

    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: query.parent_id,
                recursive: true,
                include_item_types: vec!["Episode".to_owned()],
                min_premiere_date: Some(Utc::now() - Duration::days(1)),
                order: BaseItemOrder::PremiereDateAscending,
                start_index: query.start_index,
                limit: query.limit,
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let total_record_count = usize::try_from(page.total_record_count).unwrap_or(usize::MAX);
    let start_index = usize::try_from(page.start_index).unwrap_or(usize::MAX);
    let items = project_items_to_dtos(state.as_ref(), page.items, fields, target_user_id).await?;
    Ok(Json(user_library::BaseItemQueryResult {
        items,
        total_record_count,
        start_index,
    }))
}

pub(crate) async fn next_up(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<NextUpQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let fields = user_library::BaseItemDtoFields::from_names(&query.fields);
    let parent_id = if let Some(series_id) = query.series_id {
        validate_series(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            series_id,
        )
        .await?;
        Some(series_id)
    } else {
        query.parent_id
    };

    let _ = (
        query.enable_images,
        query.image_type_limit,
        query.enable_image_types,
        query.enable_user_data,
    );
    let next_up_date_cutoff = query
        .next_up_date_cutoff
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok().map(DateTime::<Utc>::from));

    let page = state
        .user_library
        .next_up(
            &authenticated.user,
            target_user_id,
            parent_id,
            query.enable_rewatching,
            query.enable_resumable,
            next_up_date_cutoff,
            query.start_index,
            query.limit,
            query.enable_total_record_count,
        )
        .await?;
    let total_record_count = usize::try_from(page.total_record_count).unwrap_or(usize::MAX);
    let start_index = usize::try_from(page.start_index).unwrap_or(usize::MAX);
    let items = project_items_to_dtos(state.as_ref(), page.items, fields, target_user_id).await?;
    Ok(Json(user_library::BaseItemQueryResult {
        items,
        total_record_count,
        start_index,
    }))
}

pub(crate) async fn episodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<Uuid>,
    Query(query): Query<EpisodesQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let fields = user_library::BaseItemDtoFields::from_names(&query.fields);
    let order = if query
        .sort_by
        .as_deref()
        .is_some_and(|sort| sort.eq_ignore_ascii_case("Random"))
    {
        BaseItemOrder::Random
    } else {
        BaseItemOrder::SortName
    };

    let _ = (
        query.enable_images,
        query.image_type_limit,
        query.enable_image_types,
        query.enable_user_data,
    );

    let mut episodes = if let Some(season_id) = query.season_id {
        let season = state
            .user_library
            .item(&authenticated.user, target_user_id, season_id)
            .await?;
        if !season.item_type.eq_ignore_ascii_case("Season") {
            return Err(UserLibraryError::ItemNotFound.into());
        }
        query_episodes_under(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            season_id,
            false,
            order,
        )
        .await?
    } else if let Some(season_number) = query.season {
        validate_series(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            series_id,
        )
        .await?;
        let seasons = state
            .user_library
            .query_items(
                &authenticated.user,
                target_user_id,
                BaseItemQuery {
                    parent_id: Some(series_id),
                    recursive: false,
                    include_item_types: vec!["Season".to_owned()],
                    order: BaseItemOrder::SortName,
                    enable_total_record_count: Some(false),
                    ..BaseItemQuery::default()
                },
            )
            .await?;
        let Some(season) = seasons
            .items
            .into_iter()
            .find(|item| item.index_number == Some(season_number))
        else {
            return Ok(Json(user_library::BaseItemQueryResult {
                items: Vec::new(),
                total_record_count: 0,
                start_index: usize::try_from(query.start_index).unwrap_or(usize::MAX),
            }));
        };
        query_episodes_under(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            season.id,
            false,
            order,
        )
        .await?
    } else {
        validate_series(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            series_id,
        )
        .await?;
        query_episodes_under(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            series_id,
            true,
            order,
        )
        .await?
    };

    if let Some(expected) = query.is_missing {
        episodes.retain(|item| is_missing(item) == expected);
    }
    if let Some(start_item_id) = query.start_item_id {
        let start_id = state
            .user_library
            .item(&authenticated.user, target_user_id, start_item_id)
            .await
            .ok()
            .and_then(|item| item.primary_version_id)
            .unwrap_or(start_item_id);
        episodes = episodes
            .into_iter()
            .skip_while(|item| item.id != start_id)
            .collect();
    }
    if let Some(adjacent_to) = query.adjacent_to {
        episodes = filter_for_adjacency(episodes, adjacent_to);
    }

    let total_record_count = episodes.len();
    let return_items = apply_paging(episodes, query.start_index, query.limit);
    let items = project_items_to_dtos(state.as_ref(), return_items, fields, target_user_id).await?;
    Ok(Json(user_library::BaseItemQueryResult {
        items,
        total_record_count,
        start_index: usize::try_from(query.start_index).unwrap_or(usize::MAX),
    }))
}

pub(crate) async fn seasons(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<Uuid>,
    Query(query): Query<SeasonsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let series = state
        .user_library
        .item(&authenticated.user, target_user_id, series_id)
        .await?;
    if !series.item_type.eq_ignore_ascii_case("Series") {
        return Err(UserLibraryError::ItemNotFound.into());
    }

    let fields = user_library::BaseItemDtoFields::from_names(&query.fields);
    let _ = (
        query.enable_images,
        query.image_type_limit,
        query.enable_image_types,
        query.enable_user_data,
    );

    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: Some(series_id),
                recursive: false,
                include_item_types: vec!["Season".to_owned()],
                order: BaseItemOrder::SortName,
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let mut seasons = page
        .items
        .into_iter()
        .filter(|item| {
            query
                .is_special_season
                .is_none_or(|expected| is_special_season(item) == expected)
        })
        .filter(|item| {
            query
                .is_missing
                .is_none_or(|expected| is_missing(item) == expected)
        })
        .collect::<Vec<_>>();
    if let Some(adjacent_to) = query.adjacent_to {
        seasons = filter_for_adjacency(seasons, adjacent_to);
    }

    let items = project_items_to_dtos(state.as_ref(), seasons, fields, target_user_id).await?;
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}

async fn validate_series(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    series_id: Uuid,
) -> Result<(), ApiError> {
    let series = state
        .user_library
        .item(authenticated_user, target_user_id, series_id)
        .await?;
    if series.item_type.eq_ignore_ascii_case("Series") {
        Ok(())
    } else {
        Err(UserLibraryError::ItemNotFound.into())
    }
}

async fn query_episodes_under(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    parent_id: Uuid,
    recursive: bool,
    order: BaseItemOrder,
) -> Result<Vec<base_item::Model>, ApiError> {
    Ok(state
        .user_library
        .query_items(
            authenticated_user,
            target_user_id,
            BaseItemQuery {
                parent_id: Some(parent_id),
                recursive,
                include_item_types: vec!["Episode".to_owned()],
                order,
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?
        .items)
}

async fn project_items_to_dtos(
    state: &AppState,
    items: Vec<base_item::Model>,
    fields: user_library::BaseItemDtoFields,
    target_user_id: Uuid,
) -> Result<Vec<user_library::BaseItemDto>, ApiError> {
    let defaults =
        user_library::media_stream_defaults_for_user(state, target_user_id, fields).await?;
    let mut remembered_user_data = if fields.wants_media_streams() {
        state
            .user_data
            .get_preferred_for_items(target_user_id, &items)
            .await?
    } else {
        HashMap::new()
    };
    let mut trickplay_manifests =
        user_library::trickplay_manifests_for_items(state, &items, fields).await?;

    let mut dtos = Vec::with_capacity(items.len());
    for item in items {
        let item_id = item.id;
        let remembered = remembered_user_data.remove(&item_id);
        let mut dto = user_library::project_item_to_dto(
            state,
            item,
            target_user_id,
            fields.without_trickplay(),
            defaults.as_ref(),
            remembered.as_ref(),
        )
        .await?;
        user_library::attach_trickplay_manifest(
            &mut dto,
            fields,
            trickplay_manifests.remove(&item_id).unwrap_or_default(),
        );
        dtos.push(dto);
    }
    Ok(dtos)
}

fn apply_paging(
    items: Vec<base_item::Model>,
    start_index: u64,
    limit: Option<u64>,
) -> Vec<base_item::Model> {
    let start_index = usize::try_from(start_index).unwrap_or(usize::MAX);
    let limit = limit
        .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX))
        .unwrap_or(usize::MAX);
    items.into_iter().skip(start_index).take(limit).collect()
}

fn is_special_season(item: &base_item::Model) -> bool {
    item.index_number == Some(0)
}

fn is_missing(item: &base_item::Model) -> bool {
    metadata_bool(
        item.data.as_ref(),
        &[
            "IsMissing",
            "isMissing",
            "IsMissingEpisode",
            "is_missing",
            "is_missing_episode",
        ],
    )
    .unwrap_or(item.is_virtual_item)
}

fn metadata_bool(value: Option<&Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value?.as_object()?.get(*key)?.as_bool())
}

fn filter_for_adjacency(items: Vec<base_item::Model>, adjacent_to: Uuid) -> Vec<base_item::Model> {
    let Some(index) = items.iter().position(|item| item.id == adjacent_to) else {
        return Vec::new();
    };
    items
        .into_iter()
        .enumerate()
        .filter_map(|(candidate_index, item)| {
            (candidate_index == index
                || candidate_index.checked_add(1) == Some(index)
                || index.checked_add(1) == Some(candidate_index))
            .then_some(item)
        })
        .collect()
}

const fn default_enable_total_record_count() -> bool {
    true
}

const fn default_true() -> bool {
    true
}
