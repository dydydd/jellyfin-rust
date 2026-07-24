use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
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

    let _ = (
        query.fields,
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

    let items = seasons
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
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
