use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use jellyfin_data::ItemValueQuery;
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct QueryFilters {
    genres: Vec<NameGuidPair>,
    tags: Vec<String>,
    audio_languages: Vec<NameValuePair>,
    subtitle_languages: Vec<NameValuePair>,
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

fn is_music_filter(include_item_types: &[String]) -> bool {
    include_item_types.len() == 1
        && MUSIC_ITEM_TYPES
            .iter()
            .any(|item_type| include_item_types[0].eq_ignore_ascii_case(item_type))
}
