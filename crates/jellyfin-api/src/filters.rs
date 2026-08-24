use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use jellyfin_data::{BaseItemQuery, ItemValueQuery, ProductionYearOrder, entities::item_value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

const MUSIC_ITEM_TYPES: [&str; 4] = ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"];

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FiltersQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(rename = "recursive", alias = "Recursive")]
    recursive: Option<bool>,
    #[serde(
        default,
        rename = "mediaTypes",
        alias = "MediaTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct QueryFilters {
    genres: Vec<NameGuidPair>,
    tags: Vec<String>,
    audio_languages: Vec<NameValuePair>,
    subtitle_languages: Vec<NameValuePair>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct QueryFiltersLegacy {
    genres: Vec<String>,
    tags: Vec<String>,
    official_ratings: Vec<String>,
    years: Vec<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NameGuidPair {
    name: String,
    id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NameValuePair {
    name: String,
    value: String,
}

pub(crate) async fn filters2(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<QueryFilters>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let recursive = query.recursive.unwrap_or(true);
    let item_query = ItemValueQuery {
        parent_id: scoped_parent_id(&query),
        recursive,
        include_item_types: query.include_item_types.clone(),
        user_id: Some(target_user_id),
        ..ItemValueQuery::default()
    };
    let genres = if is_music_filter(&query.include_item_types) {
        state
            .music_genres
            .list(&authenticated.user, target_user_id, item_query)
            .await?
            .genres
            .into_iter()
            .map(|genre| NameGuidPair {
                name: genre.name,
                id: genre.id,
            })
            .collect()
    } else {
        state
            .genres
            .list(&authenticated.user, target_user_id, item_query)
            .await?
            .genres
            .into_iter()
            .map(|genre| NameGuidPair {
                name: genre.name,
                id: genre.id,
            })
            .collect()
    };
    Ok(Json(QueryFilters {
        genres,
        tags: Vec::new(),
        audio_languages: Vec::new(),
        subtitle_languages: Vec::new(),
    }))
}

pub(crate) async fn filters_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<QueryFiltersLegacy>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = target_user_id(&state, &authenticated.user, query.user_id).await?;
    let Some(parent_id) = legacy_parent_id(&state, &query).await? else {
        return Ok(Json(QueryFiltersLegacy::default()));
    };

    let item_query = BaseItemQuery {
        parent_id: Some(parent_id),
        recursive: true,
        include_item_types: query.include_item_types.clone(),
        media_types: query.media_types.clone(),
        user_id: Some(target_user_id),
        ..BaseItemQuery::default()
    };
    let value_query = ItemValueQuery {
        parent_id: Some(parent_id),
        recursive: true,
        include_item_types: query.include_item_types,
        media_types: query.media_types,
        user_id: Some(target_user_id),
        ..ItemValueQuery::default()
    };

    let years = state
        .base_items
        .production_years(&item_query, ProductionYearOrder::Ascending)
        .await?
        .years;
    let official_ratings = state.base_items.official_ratings(&item_query).await?;
    let tags = state
        .item_values
        .query_values(item_value::ItemValueType::Tags, &value_query)
        .await
        .map_err(|_| ApiError::Internal)?
        .values
        .into_iter()
        .map(|value| value.value)
        .collect();
    let genres = state
        .item_values
        .query_values(item_value::ItemValueType::Genre, &value_query)
        .await
        .map_err(|_| ApiError::Internal)?
        .values
        .into_iter()
        .map(|value| value.value)
        .collect();

    Ok(Json(QueryFiltersLegacy {
        genres,
        tags,
        official_ratings,
        years,
    }))
}

fn scoped_parent_id(query: &FiltersQuery) -> Option<Uuid> {
    if query.include_item_types.len() == 1
        && ["Trailer", "Program"]
            .iter()
            .any(|item_type| query.include_item_types[0].eq_ignore_ascii_case(item_type))
    {
        None
    } else {
        query.parent_id
    }
}

async fn legacy_parent_id(
    state: &AppState,
    query: &FiltersQuery,
) -> Result<Option<Uuid>, ApiError> {
    if query.include_item_types.len() == 1
        && ["Trailer", "Program"]
            .iter()
            .any(|item_type| query.include_item_types[0].eq_ignore_ascii_case(item_type))
    {
        return Ok(None);
    }

    let items = &state.base_items;
    let parent = if let Some(parent_id) = query.parent_id {
        items.get(parent_id).await?
    } else {
        Some(items.ensure_user_root().await?)
    };
    Ok(parent.filter(|item| item.is_folder).map(|folder| folder.id))
}

async fn target_user_id(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    requested: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let requested = requested.filter(|user_id| !user_id.is_nil());
    let target_user_id = match requested {
        Some(user_id)
            if user_id != authenticated_user.id && !authenticated_user.is_administrator =>
        {
            return Err(ApiError::Forbidden);
        }
        Some(user_id) => user_id,
        None => authenticated_user.id,
    };
    state.users.get(target_user_id).await?;
    Ok(target_user_id)
}

fn is_music_filter(include_item_types: &[String]) -> bool {
    include_item_types.len() == 1
        && MUSIC_ITEM_TYPES
            .iter()
            .any(|item_type| include_item_types[0].eq_ignore_ascii_case(item_type))
}
