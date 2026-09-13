use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
    response::Response,
};
use axum_extra::extract::Query;
use jellyfin_controller::{ArtistValueKind, UserError};
use jellyfin_data::{BaseItemPage, ItemValueQuery};
use serde::Deserialize;
use std::str::FromStr;
use uuid::Uuid;

use crate::item_images::{GetItemImageQuery, parse_image_type, render_item_image};
use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArtistsQuery {
    #[serde(
        default,
        rename = "minCommunityRating",
        alias = "MinCommunityRating",
        alias = "mincommunityrating"
    )]
    _min_community_rating: Option<f64>,
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: i32,
    #[serde(rename = "limit", alias = "Limit")]
    limit: Option<i32>,
    #[serde(rename = "searchTerm", alias = "SearchTerm", alias = "searchterm")]
    search_term: Option<String>,
    #[serde(rename = "parentId", alias = "ParentId", alias = "parentid")]
    parent_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(
        rename = "enableUserData",
        alias = "EnableUserData",
        alias = "enableuserdata"
    )]
    enable_user_data: Option<bool>,
    #[serde(
        rename = "imageTypeLimit",
        alias = "ImageTypeLimit",
        alias = "imagetypelimit"
    )]
    image_type_limit: Option<i32>,
    #[serde(
        default,
        rename = "enableImageTypes",
        alias = "EnableImageTypes",
        alias = "enableimagetypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    enable_image_types: Vec<String>,
    #[serde(
        rename = "enableImages",
        alias = "EnableImages",
        alias = "enableimages"
    )]
    enable_images: Option<bool>,
    #[serde(
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        alias = "includeitemtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    filters: Vec<ArtistItemFilter>,
    #[serde(
        default,
        rename = "excludeItemTypes",
        alias = "ExcludeItemTypes",
        alias = "excludeitemtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_types: Vec<String>,
    #[serde(
        default,
        rename = "mediaTypes",
        alias = "MediaTypes",
        alias = "mediatypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
    #[serde(
        default,
        rename = "genres",
        alias = "Genres",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    genres: Vec<String>,
    #[serde(
        default,
        rename = "genreIds",
        alias = "GenreIds",
        alias = "genreids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    genre_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "officialRatings",
        alias = "OfficialRatings",
        alias = "officialratings",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    official_ratings: Vec<String>,
    #[serde(
        default,
        rename = "tags",
        alias = "Tags",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    tags: Vec<String>,
    #[serde(
        default,
        rename = "years",
        alias = "Years",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    years: Vec<i32>,
    #[serde(
        default,
        rename = "studios",
        alias = "Studios",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    studios: Vec<String>,
    #[serde(
        default,
        rename = "studioIds",
        alias = "StudioIds",
        alias = "studioids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    studio_ids: Vec<Uuid>,
    #[serde(default, rename = "person", alias = "Person")]
    _person: Option<String>,
    #[serde(
        default,
        rename = "personIds",
        alias = "PersonIds",
        alias = "personids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    _person_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "personTypes",
        alias = "PersonTypes",
        alias = "persontypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    _person_types: Vec<String>,
    #[serde(
        default,
        rename = "isFavorite",
        alias = "IsFavorite",
        alias = "isfavorite"
    )]
    is_favorite: Option<bool>,
    #[serde(
        rename = "nameStartsWithOrGreater",
        alias = "NameStartsWithOrGreater",
        alias = "namestartswithorgreater"
    )]
    name_starts_with_or_greater: Option<String>,
    #[serde(
        rename = "nameStartsWith",
        alias = "NameStartsWith",
        alias = "namestartswith"
    )]
    name_starts_with: Option<String>,
    #[serde(
        rename = "nameLessThan",
        alias = "NameLessThan",
        alias = "namelessthan"
    )]
    name_less_than: Option<String>,
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
    #[serde(default = "default_total_record_count")]
    #[serde(
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount",
        alias = "enabletotalrecordcount"
    )]
    enable_total_record_count: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArtistByNameQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    list_kind(state, headers, &uri, query, ArtistValueKind::Artist).await
}

pub(crate) async fn list_album_artists(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    list_kind(state, headers, &uri, query, ArtistValueKind::AlbumArtist).await
}

async fn list_kind(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: &axum::http::Uri,
    query: ArtistsQuery,
    kind: ArtistValueKind,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    // Jellyfin's nullable Int32 values are passed through to the response. Its
    // item-by-name repository only skips positive offsets, and SQLite treats a
    // negative LIMIT as unlimited.
    let requested_start_index = query.start_index;
    let start_index = u64::try_from(requested_start_index).unwrap_or_default();
    let limit = query.limit.and_then(|limit| u64::try_from(limit).ok());
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let order = crate::query::item_value_order(&query.sort_by)?;
    let descending = descending(&query.sort_order)?;
    let enable_total_record_count = query.enable_total_record_count;
    let include_item_counts = user_library::BaseItemDtoFields::from_names(&query.fields)
        .wants_item_counts()
        || !query.include_item_types.is_empty();
    let mut item_query = ItemValueQuery {
        parent_id: query.parent_id,
        search_term: query.search_term,
        include_item_types: query.include_item_types,
        exclude_item_types: query.exclude_item_types,
        media_types: query.media_types,
        genres: query.genres,
        genre_ids: query.genre_ids,
        official_ratings: query.official_ratings,
        tags: query.tags,
        years: query.years,
        studio_ids: if query.studios.is_empty() {
            query.studio_ids
        } else {
            Vec::new()
        },
        studios: query.studios,
        is_favorite: query.is_favorite,
        user_id: Some(target_user_id),
        name_starts_with_or_greater: query.name_starts_with_or_greater,
        name_starts_with: query.name_starts_with,
        name_less_than: query.name_less_than,
        start_index,
        limit,
        order,
        descending,
        enable_total_record_count: Some(enable_total_record_count),
        ..ItemValueQuery::default()
    };
    apply_filters(&mut item_query, &query.filters)?;
    state
        .user_library
        .apply_item_value_policy(&authenticated.user, target_user_id, &mut item_query)
        .await?;
    let page = state.artists.list_authorized(kind, item_query).await?;
    let artist_ids = page
        .artists
        .iter()
        .map(|artist| artist.id)
        .collect::<Vec<_>>();
    let mut persisted_by_id = state
        .base_items
        .get_many(&artist_ids)
        .await?
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<std::collections::HashMap<_, _>>();
    let persisted_artists = page
        .artists
        .iter()
        .filter_map(|artist| persisted_by_id.remove(&artist.id))
        .collect::<Vec<_>>();
    let persisted_artist_ids = persisted_artists
        .iter()
        .map(|artist| artist.id)
        .collect::<Vec<_>>();
    let dto_options = crate::items::PageDtoOptions {
        enable_images: query.enable_images.unwrap_or(true),
        image_type_limit: query
            .image_type_limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0)),
        enable_image_types: crate::items::parse_image_type_selectors(&query.enable_image_types),
        enable_user_data: query.enable_user_data.unwrap_or(true),
    };
    let projected = crate::items::page_to_dto_with_options(
        state.as_ref(),
        BaseItemPage {
            items: persisted_artists,
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        },
        query.fields,
        target_user_id,
        &dto_options,
    )
    .await?;
    let mut projected_by_id = persisted_artist_ids
        .into_iter()
        .zip(projected.items)
        .collect::<std::collections::HashMap<_, _>>();
    let mut items = Vec::with_capacity(page.artists.len());
    for artist in page.artists {
        let mut dto = if let Some(dto) = projected_by_id.remove(&artist.id) {
            dto
        } else {
            let mut dto = user_library::artist_to_dto(
                artist.clone(),
                state.server_id(),
                include_item_counts,
            )?;
            if !dto_options.enable_images {
                dto.image_tags = None;
                dto.backdrop_image_tags = None;
            } else if dto_options.image_type_limit == 0
                || (!dto_options.enable_image_types.is_empty()
                    && !dto_options.enable_image_types.contains(&2))
            {
                dto.backdrop_image_tags = None;
            }
            dto
        };
        // Item-by-name artist list entries retain their historical folder
        // shape even when the accessed-by-name backing row is non-folder.
        dto.is_folder = true;
        if include_item_counts {
            apply_artist_counts(&mut dto, artist.item_count, artist.counts)?;
        }
        items.push(dto);
    }
    let total_record_count = if enable_total_record_count {
        user_library::checked_int32(page.total_record_count)?
    } else {
        user_library::checked_int32(items.len())?
    };
    let mut result = user_library::BaseItemQueryResult {
        items,
        total_record_count,
        start_index: requested_start_index,
    };
    user_library::omit_incompatible_emby_relations(uri, &mut result.items);
    Ok(Json(result))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(name): Path<String>,
    Query(query): Query<ArtistByNameQuery>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    if target_user_id != authenticated.user.id && !authenticated.user.is_administrator {
        return Err(ApiError::Forbidden);
    }
    let target_user_exists = match state.users.get(target_user_id).await {
        Ok(_) => true,
        Err(UserError::NotFound) if authenticated.user.is_administrator => false,
        Err(error) => return Err(error.into()),
    };
    let mut item_query = ItemValueQuery::default();
    if target_user_exists {
        state
            .user_library
            .apply_item_value_policy(&authenticated.user, target_user_id, &mut item_query)
            .await?;
    }
    let detail = state
        .artists
        .get(&authenticated.user, target_user_id, &name, item_query)
        .await?;
    let projection_user_id = if target_user_exists {
        target_user_id
    } else {
        authenticated.user.id
    };
    let mut dto = user_library::project_item_to_dto(
        &state,
        detail.item,
        projection_user_id,
        user_library::BaseItemDtoFields::all(),
        None,
        None,
    )
    .await?;
    if !target_user_exists {
        dto.user_data = None;
    }
    apply_artist_counts(&mut dto, detail.item_count, detail.counts)?;
    user_library::omit_incompatible_emby_relations(&uri, std::slice::from_mut(&mut dto));
    Ok(Json(dto))
}

fn apply_artist_counts(
    dto: &mut user_library::BaseItemDto,
    item_count: u64,
    counts: jellyfin_data::ItemValueCounts,
) -> Result<(), ApiError> {
    dto.child_count = Some(user_library::checked_int32(item_count)?);
    dto.album_count = Some(user_library::checked_int32(counts.album_count)?);
    dto.music_video_count = Some(user_library::checked_int32(counts.music_video_count)?);
    dto.song_count = Some(user_library::checked_int32(counts.song_count)?);
    Ok(())
}

pub(crate) async fn get_image(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type, image_index)): Path<(String, String, i32)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    get_image_for(state, uri, headers, name, image_type, image_index, query).await
}

pub(crate) async fn get_image_default(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type)): Path<(String, String)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    get_image_for(state, uri, headers, name, image_type, 0, query).await
}

async fn get_image_for(
    state: Arc<AppState>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    name: String,
    image_type: String,
    image_index: i32,
    query: GetItemImageQuery,
) -> Result<Response, ApiError> {
    authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    let image_type = parse_image_type(&image_type)?;
    let item = state
        .artists
        .image_item(&name)
        .await?
        .ok_or(jellyfin_controller::ArtistError::NotFound)?;
    render_item_image(&state, &headers, item.id, image_type, image_index, query).await
}

fn descending(sort_order: &[String]) -> Result<bool, ApiError> {
    let Some(order) = sort_order.first() else {
        return Ok(false);
    };
    if order.eq_ignore_ascii_case("Descending") {
        Ok(true)
    } else if order.eq_ignore_ascii_case("Ascending") {
        Ok(false)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

const fn default_total_record_count() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtistItemFilter {
    IsFolder,
    IsNotFolder,
    IsUnplayed,
    IsPlayed,
    IsFavorite,
    IsResumable,
    Likes,
    Dislikes,
    IsFavoriteOrLikes,
}

impl FromStr for ArtistItemFilter {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "1" => Ok(Self::IsFolder),
            "2" => Ok(Self::IsNotFolder),
            "3" => Ok(Self::IsUnplayed),
            "4" => Ok(Self::IsPlayed),
            "5" => Ok(Self::IsFavorite),
            "7" => Ok(Self::IsResumable),
            "8" => Ok(Self::Likes),
            "9" => Ok(Self::Dislikes),
            "10" => Ok(Self::IsFavoriteOrLikes),
            name if name.eq_ignore_ascii_case("IsFolder") => Ok(Self::IsFolder),
            name if name.eq_ignore_ascii_case("IsNotFolder") => Ok(Self::IsNotFolder),
            name if name.eq_ignore_ascii_case("IsUnplayed") => Ok(Self::IsUnplayed),
            name if name.eq_ignore_ascii_case("IsPlayed") => Ok(Self::IsPlayed),
            name if name.eq_ignore_ascii_case("IsFavorite") => Ok(Self::IsFavorite),
            name if name.eq_ignore_ascii_case("IsResumable") => Ok(Self::IsResumable),
            name if name.eq_ignore_ascii_case("Likes") => Ok(Self::Likes),
            name if name.eq_ignore_ascii_case("Dislikes") => Ok(Self::Dislikes),
            name if name.eq_ignore_ascii_case("IsFavoriteOrLikes") => Ok(Self::IsFavoriteOrLikes),
            _ => Err(()),
        }
    }
}

fn apply_filters(query: &mut ItemValueQuery, filters: &[ArtistItemFilter]) -> Result<(), ApiError> {
    if (filters.contains(&ArtistItemFilter::IsFolder)
        && filters.contains(&ArtistItemFilter::IsNotFolder))
        || (filters.contains(&ArtistItemFilter::IsPlayed)
            && filters.contains(&ArtistItemFilter::IsUnplayed))
        || (filters.contains(&ArtistItemFilter::Likes)
            && filters.contains(&ArtistItemFilter::Dislikes))
    {
        return Err(ApiError::InvalidRequest);
    }
    for filter in filters {
        match filter {
            // Jellyfin's item-by-name artist query accepts these flags, but
            // intentionally omits them from the outer MusicArtist filter.
            ArtistItemFilter::IsFolder
            | ArtistItemFilter::IsNotFolder
            | ArtistItemFilter::IsResumable => {}
            ArtistItemFilter::IsUnplayed => query.is_played = Some(false),
            ArtistItemFilter::IsPlayed => query.is_played = Some(true),
            ArtistItemFilter::IsFavorite => query.is_favorite = Some(true),
            ArtistItemFilter::Likes => query.is_liked = Some(true),
            ArtistItemFilter::Dislikes => query.is_liked = Some(false),
            ArtistItemFilter::IsFavoriteOrLikes => query.is_favorite_or_liked = Some(true),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ArtistItemFilter, apply_filters};
    use jellyfin_data::ItemValueQuery;
    use std::str::FromStr;

    #[test]
    fn artist_filters_accept_official_names_and_integer_values() {
        assert_eq!(
            ArtistItemFilter::from_str("isfavorite").unwrap(),
            ArtistItemFilter::IsFavorite
        );
        assert_eq!(
            ArtistItemFilter::from_str("10").unwrap(),
            ArtistItemFilter::IsFavoriteOrLikes
        );
        assert!(ArtistItemFilter::from_str("6").is_err());
    }

    #[test]
    fn artist_filters_reject_official_conflicting_pairs() {
        for filters in [
            [ArtistItemFilter::IsFolder, ArtistItemFilter::IsNotFolder],
            [ArtistItemFilter::IsPlayed, ArtistItemFilter::IsUnplayed],
            [ArtistItemFilter::Likes, ArtistItemFilter::Dislikes],
        ] {
            assert!(apply_filters(&mut ItemValueQuery::default(), &filters).is_err());
        }
    }
}
