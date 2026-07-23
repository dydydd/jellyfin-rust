use std::{str::FromStr, sync::Arc};

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::Query;
use jellyfin_controller::{Artist, ArtistValueKind, Genre, Person, Studio};
use jellyfin_data::{
    BaseItemPage, BaseItemQuery, ItemValueQuery, PersonQuery, entities::base_item,
};
use jellyfin_model::{MediaType, SearchHint, SearchHintResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SearchHintsQuery {
    #[serde(rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    search_term: Option<String>,
    #[serde(
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(
        rename = "excludeItemTypes",
        alias = "ExcludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_types: Vec<String>,
    #[serde(
        rename = "mediaTypes",
        alias = "MediaTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(rename = "isMovie", alias = "IsMovie")]
    is_movie: Option<bool>,
    #[serde(rename = "isSeries", alias = "IsSeries")]
    is_series: Option<bool>,
    #[serde(rename = "includePeople", alias = "IncludePeople")]
    include_people: Option<bool>,
    #[serde(rename = "includeMedia", alias = "IncludeMedia")]
    include_media: Option<bool>,
    #[serde(rename = "includeGenres", alias = "IncludeGenres")]
    include_genres: Option<bool>,
    #[serde(rename = "includeStudios", alias = "IncludeStudios")]
    include_studios: Option<bool>,
    #[serde(rename = "includeArtists", alias = "IncludeArtists")]
    include_artists: Option<bool>,
}

pub(crate) async fn hints(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SearchHintsQuery>,
) -> Result<Json<SearchHintResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let search_term = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let mut result = SearchHintResult::default();

    if query.include_media.unwrap_or(true) {
        let page = state
            .user_library
            .query_items(
                &authenticated.user,
                target_user_id,
                BaseItemQuery {
                    parent_id: query.parent_id,
                    recursive: true,
                    search_term: Some(search_term.to_owned()),
                    include_item_types: query.include_item_types.clone(),
                    exclude_item_types: query.exclude_item_types.clone(),
                    media_types: query.media_types.clone(),
                    is_movie: query.is_movie,
                    is_series: query.is_series,
                    is_virtual_item: Some(false),
                    start_index: query.start_index,
                    limit: query.limit,
                    ..BaseItemQuery::default()
                },
            )
            .await?;
        let media = media_search_hint_result(page, search_term);
        result.total_record_count += media.total_record_count;
        result.search_hints.extend(media.search_hints);
    }

    if query.include_people.unwrap_or(true) {
        let page = state
            .persons
            .list(
                &authenticated.user,
                target_user_id,
                PersonQuery {
                    parent_id: query.parent_id,
                    recursive: true,
                    search_term: Some(search_term.to_owned()),
                    include_item_types: query.include_item_types.clone(),
                    exclude_item_types: query.exclude_item_types.clone(),
                    media_types: query.media_types.clone(),
                    is_movie: query.is_movie,
                    is_series: query.is_series,
                    user_id: Some(target_user_id),
                    start_index: query.start_index,
                    limit: query.limit,
                    ..PersonQuery::default()
                },
            )
            .await?;
        result.total_record_count += usize::try_from(page.total_record_count).unwrap_or(usize::MAX);
        result.search_hints.extend(
            page.people
                .into_iter()
                .map(|person| person_hint(person, search_term)),
        );
    }

    if query.include_genres.unwrap_or(true) {
        let page = state
            .genres
            .list(
                &authenticated.user,
                target_user_id,
                ItemValueQuery {
                    parent_id: query.parent_id,
                    recursive: true,
                    search_term: Some(search_term.to_owned()),
                    include_item_types: query.include_item_types.clone(),
                    exclude_item_types: query.exclude_item_types.clone(),
                    media_types: query.media_types.clone(),
                    is_movie: query.is_movie,
                    is_series: query.is_series,
                    user_id: Some(target_user_id),
                    start_index: query.start_index,
                    limit: query.limit,
                    ..ItemValueQuery::default()
                },
            )
            .await?;
        result.total_record_count += usize::try_from(page.total_record_count).unwrap_or(usize::MAX);
        result.search_hints.extend(
            page.genres
                .into_iter()
                .map(|genre| genre_hint(genre, search_term)),
        );
    }

    if query.include_studios.unwrap_or(true) {
        let page = state
            .studios
            .list(
                &authenticated.user,
                target_user_id,
                ItemValueQuery {
                    parent_id: query.parent_id,
                    recursive: true,
                    search_term: Some(search_term.to_owned()),
                    include_item_types: query.include_item_types.clone(),
                    exclude_item_types: query.exclude_item_types.clone(),
                    media_types: query.media_types.clone(),
                    is_movie: query.is_movie,
                    is_series: query.is_series,
                    user_id: Some(target_user_id),
                    start_index: query.start_index,
                    limit: query.limit,
                    ..ItemValueQuery::default()
                },
            )
            .await?;
        result.total_record_count += usize::try_from(page.total_record_count).unwrap_or(usize::MAX);
        result.search_hints.extend(
            page.studios
                .into_iter()
                .map(|studio| studio_hint(studio, search_term)),
        );
    }

    if query.include_artists.unwrap_or(true) {
        let page = state
            .artists
            .list(
                &authenticated.user,
                target_user_id,
                ArtistValueKind::Artist,
                ItemValueQuery {
                    parent_id: query.parent_id,
                    recursive: true,
                    search_term: Some(search_term.to_owned()),
                    include_item_types: query.include_item_types,
                    exclude_item_types: query.exclude_item_types,
                    media_types: query.media_types,
                    is_movie: query.is_movie,
                    is_series: query.is_series,
                    user_id: Some(target_user_id),
                    start_index: query.start_index,
                    limit: query.limit,
                    ..ItemValueQuery::default()
                },
            )
            .await?;
        result.total_record_count += usize::try_from(page.total_record_count).unwrap_or(usize::MAX);
        result.search_hints.extend(
            page.artists
                .into_iter()
                .map(|artist| artist_hint(artist, search_term)),
        );
    }

    Ok(Json(result))
}

fn media_search_hint_result(page: BaseItemPage, matched_term: &str) -> SearchHintResult {
    SearchHintResult {
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        search_hints: page
            .items
            .into_iter()
            .map(|item| search_hint(item, matched_term))
            .collect(),
    }
}

fn search_hint(item: base_item::Model, matched_term: &str) -> SearchHint {
    SearchHint {
        item_id: item.id,
        id: item.id,
        name: item.name.unwrap_or_default(),
        matched_term: Some(matched_term.to_owned()),
        index_number: item.index_number,
        production_year: item.production_year,
        parent_index_number: item.parent_index_number,
        item_type: item.item_type,
        is_folder: item.is_folder.then_some(true),
        run_time_ticks: item.runtime_ticks,
        media_type: item
            .media_type
            .as_deref()
            .and_then(|media_type| MediaType::from_str(media_type).ok())
            .unwrap_or(MediaType::Unknown),
        artists: metadata_string_array(item.data.as_ref(), &["Artists", "artists"]),
        album: metadata_string(item.data.as_ref(), &["Album", "album"]),
        album_artist: metadata_string(item.data.as_ref(), &["AlbumArtist", "album_artist"]),
        series: metadata_string(item.data.as_ref(), &["Series", "SeriesName", "series"]),
        ..SearchHint::default()
    }
}

fn genre_hint(genre: Genre, matched_term: &str) -> SearchHint {
    SearchHint {
        item_id: genre.id,
        id: genre.id,
        name: genre.name,
        matched_term: Some(matched_term.to_owned()),
        item_type: "Genre".to_owned(),
        is_folder: Some(true),
        ..SearchHint::default()
    }
}

fn person_hint(person: Person, matched_term: &str) -> SearchHint {
    SearchHint {
        item_id: person.model.id,
        id: person.model.id,
        name: person.model.name,
        matched_term: Some(matched_term.to_owned()),
        item_type: "Person".to_owned(),
        ..SearchHint::default()
    }
}

fn studio_hint(studio: Studio, matched_term: &str) -> SearchHint {
    SearchHint {
        item_id: studio.id,
        id: studio.id,
        name: studio.name,
        matched_term: Some(matched_term.to_owned()),
        item_type: "Studio".to_owned(),
        is_folder: Some(true),
        ..SearchHint::default()
    }
}

fn artist_hint(artist: Artist, matched_term: &str) -> SearchHint {
    SearchHint {
        item_id: artist.id,
        id: artist.id,
        name: artist.name,
        matched_term: Some(matched_term.to_owned()),
        item_type: "MusicArtist".to_owned(),
        is_folder: Some(true),
        ..SearchHint::default()
    }
}

fn metadata_string(value: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let object = value?.as_object()?;
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| value.as_str().map(str::to_owned))
}

fn metadata_string_array(value: Option<&serde_json::Value>, keys: &[&str]) -> Vec<String> {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find_map(|value| {
            value.as_array().map(|array| {
                array
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default()
}
