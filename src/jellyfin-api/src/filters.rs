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
    #[serde(rename = "isAiring", alias = "IsAiring", alias = "isairing")]
    is_airing: Option<bool>,
    #[serde(rename = "isMovie", alias = "IsMovie", alias = "ismovie")]
    is_movie: Option<bool>,
    #[serde(rename = "isSports", alias = "IsSports", alias = "issports")]
    is_sports: Option<bool>,
    #[serde(rename = "isKids", alias = "IsKids", alias = "iskids")]
    is_kids: Option<bool>,
    #[serde(rename = "isNews", alias = "IsNews", alias = "isnews")]
    is_news: Option<bool>,
    #[serde(rename = "isSeries", alias = "IsSeries", alias = "isseries")]
    is_series: Option<bool>,
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
    let access_policy = state
        .user_library
        .filter_access_policy(&authenticated.user, target_user_id)
        .await?;
    let recursive = query.recursive.unwrap_or(true);
    let parent_id = scoped_parent_id(&query);
    let is_music_filter = is_music_filter(&query.include_item_types);
    let stream_language_item_types = stream_language_item_types(&query.include_item_types);
    let item_query = ItemValueQuery {
        parent_id,
        recursive,
        include_item_types: query.include_item_types,
        is_airing: query.is_airing,
        is_movie: query.is_movie,
        is_sports: query.is_sports,
        is_kids: query.is_kids,
        is_news: query.is_news,
        is_series: query.is_series,
        user_id: Some(target_user_id),
        access_policy: access_policy.clone(),
        ..ItemValueQuery::default()
    };
    let genres = if is_music_filter {
        state
            .music_genres
            .list_authorized(item_query)
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
            .list_authorized(item_query)
            .await?
            .genres
            .into_iter()
            .map(|genre| NameGuidPair {
                name: genre.name,
                id: genre.id,
            })
            .collect()
    };
    let (audio_languages, subtitle_languages) =
        if let Some(include_item_types) = stream_language_item_types {
            let stream_query = BaseItemQuery {
                parent_id,
                recursive,
                include_item_types,
                is_airing: query.is_airing,
                is_movie: query.is_movie,
                is_sports: query.is_sports,
                is_kids: query.is_kids,
                is_news: query.is_news,
                is_series: query.is_series,
                ..access_policy
            };
            let languages = state
                .base_items
                .media_stream_languages_including_owned(&stream_query)
                .await?;
            (
                localized_languages(&state, languages.audio),
                localized_languages(&state, languages.subtitles),
            )
        } else {
            (Vec::new(), Vec::new())
        };
    Ok(Json(QueryFilters {
        genres,
        tags: Vec::new(),
        audio_languages,
        subtitle_languages,
    }))
}

pub(crate) async fn filters_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<QueryFiltersLegacy>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let access_policy = state
        .user_library
        .filter_access_policy(&authenticated.user, target_user_id)
        .await?;
    let Some(parent_id) = legacy_parent_id(&state, &query).await? else {
        return Ok(Json(QueryFiltersLegacy::default()));
    };

    let item_query = BaseItemQuery {
        parent_id: Some(parent_id),
        recursive: true,
        include_item_types: query.include_item_types,
        media_types: query.media_types,
        ..access_policy.clone()
    };

    let years = state
        .base_items
        .production_years(&item_query, ProductionYearOrder::Ascending)
        .await?
        .years;
    let official_ratings = state.base_items.official_ratings(&item_query).await?;
    let value_query = ItemValueQuery {
        parent_id: Some(parent_id),
        recursive: true,
        include_item_types: item_query.include_item_types,
        media_types: item_query.media_types,
        user_id: Some(target_user_id),
        access_policy,
        ..ItemValueQuery::default()
    };
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

fn is_music_filter(include_item_types: &[String]) -> bool {
    include_item_types.len() == 1
        && MUSIC_ITEM_TYPES
            .iter()
            .any(|item_type| include_item_types[0].eq_ignore_ascii_case(item_type))
}

fn stream_language_item_types(include_item_types: &[String]) -> Option<Vec<String>> {
    if !include_item_types.iter().any(|item_type| {
        ["Movie", "Series", "Season", "Episode"]
            .iter()
            .any(|video_type| item_type.eq_ignore_ascii_case(video_type))
    }) {
        return None;
    }

    let mut stream_item_types = include_item_types.to_vec();
    let includes_series_or_season = stream_item_types.iter().any(|item_type| {
        item_type.eq_ignore_ascii_case("Series") || item_type.eq_ignore_ascii_case("Season")
    });
    let includes_episode = stream_item_types
        .iter()
        .any(|item_type| item_type.eq_ignore_ascii_case("Episode"));
    if includes_series_or_season && !includes_episode {
        stream_item_types.push("Episode".to_owned());
    }
    Some(stream_item_types)
}

fn localized_languages(state: &AppState, languages: Vec<String>) -> Vec<NameValuePair> {
    let mut languages = languages
        .into_iter()
        .map(|value| {
            let name = state.localization.find_language_info(&value).map_or_else(
                || value.clone(),
                |culture| format!("{} ({value})", culture.display_name),
            );
            NameValuePair { name, value }
        })
        .collect::<Vec<_>>();
    languages.sort_unstable_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.value.cmp(&right.value))
    });
    languages
}
