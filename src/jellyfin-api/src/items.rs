use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use chrono::{DateTime, Utc};
use jellyfin_controller::SearchProviderQuery;
use jellyfin_data::{BaseItemOrder, BaseItemPage, BaseItemQuery};
use jellyfin_model::{SortOrder, UserConfiguration};
use serde::Deserialize;
use std::str::FromStr;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    recursive: Option<bool>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    search_term: Option<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(default, rename = "isPlayed", alias = "IsPlayed")]
    is_played: Option<bool>,
    #[serde(default, rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
    #[serde(default, rename = "minOfficialRating", alias = "MinOfficialRating")]
    min_official_rating: Option<String>,
    #[serde(default, rename = "maxOfficialRating", alias = "MaxOfficialRating")]
    max_official_rating: Option<String>,
    #[serde(default, rename = "hasThemeSong", alias = "HasThemeSong")]
    has_theme_song: Option<bool>,
    #[serde(default, rename = "hasThemeVideo", alias = "HasThemeVideo")]
    has_theme_video: Option<bool>,
    #[serde(default, rename = "hasSubtitles", alias = "HasSubtitles")]
    has_subtitles: Option<bool>,
    #[serde(default, rename = "hasSpecialFeature", alias = "HasSpecialFeature")]
    has_special_feature: Option<bool>,
    #[serde(default, rename = "hasTrailer", alias = "HasTrailer")]
    has_trailer: Option<bool>,
    #[serde(default, rename = "adjacentTo", alias = "AdjacentTo")]
    adjacent_to: Option<Uuid>,
    #[serde(default, rename = "indexNumber", alias = "IndexNumber")]
    index_number: Option<i32>,
    #[serde(default, rename = "parentIndexNumber", alias = "ParentIndexNumber")]
    parent_index_number: Option<i32>,
    #[serde(default, rename = "hasParentalRating", alias = "HasParentalRating")]
    has_parental_rating: Option<bool>,
    #[serde(default, rename = "isHd", alias = "IsHD")]
    is_hd: Option<bool>,
    #[serde(default, rename = "is4K", alias = "Is4K")]
    is_4k: Option<bool>,
    #[serde(
        default,
        rename = "locationTypes",
        alias = "LocationTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    location_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeLocationTypes",
        alias = "ExcludeLocationTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_location_types: Vec<String>,
    #[serde(default, rename = "isMissing", alias = "IsMissing")]
    is_missing: Option<bool>,
    #[serde(default, rename = "isUnaired", alias = "IsUnaired")]
    is_unaired: Option<bool>,
    #[serde(default, rename = "minCriticRating", alias = "MinCriticRating")]
    min_critic_rating: Option<f64>,
    #[serde(default, rename = "minPremiereDate", alias = "MinPremiereDate")]
    min_premiere_date: Option<DateTime<Utc>>,
    #[serde(default, rename = "maxPremiereDate", alias = "MaxPremiereDate")]
    max_premiere_date: Option<DateTime<Utc>>,
    #[serde(default, rename = "minDateLastSaved", alias = "MinDateLastSaved")]
    min_date_last_saved: Option<DateTime<Utc>>,
    #[serde(
        default,
        rename = "minDateLastSavedForUser",
        alias = "MinDateLastSavedForUser"
    )]
    min_date_last_saved_for_user: Option<DateTime<Utc>>,
    #[serde(default, rename = "hasOverview", alias = "HasOverview")]
    has_overview: Option<bool>,
    #[serde(default, rename = "hasImdbId", alias = "HasImdbId")]
    has_imdb_id: Option<bool>,
    #[serde(default, rename = "hasTmdbId", alias = "HasTmdbId")]
    has_tmdb_id: Option<bool>,
    #[serde(default, rename = "hasTvdbId", alias = "HasTvdbId")]
    has_tvdb_id: Option<bool>,
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    filters: Vec<ItemFilter>,
    #[serde(
        default,
        rename = "genres",
        alias = "Genres",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    genres: Vec<String>,
    #[serde(
        default,
        rename = "years",
        alias = "Years",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    years: Vec<i32>,
    #[serde(
        default,
        rename = "tags",
        alias = "Tags",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    tags: Vec<String>,
    #[serde(default, rename = "person", alias = "Person")]
    person: Option<String>,
    #[serde(
        default,
        rename = "personIds",
        alias = "PersonIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    person_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "personTypes",
        alias = "PersonTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    person_types: Vec<String>,
    #[serde(
        default,
        rename = "officialRatings",
        alias = "OfficialRatings",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    official_ratings: Vec<String>,
    #[serde(
        default,
        rename = "studios",
        alias = "Studios",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    studios: Vec<String>,
    #[serde(
        default,
        rename = "artists",
        alias = "Artists",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    artists: Vec<String>,
    #[serde(
        default,
        rename = "excludeArtistIds",
        alias = "ExcludeArtistIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "artistIds",
        alias = "ArtistIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "albumArtistIds",
        alias = "AlbumArtistIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    album_artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "contributingArtistIds",
        alias = "ContributingArtistIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    contributing_artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "albums",
        alias = "Albums",
        deserialize_with = "crate::query::pipe::deserialize"
    )]
    albums: Vec<String>,
    #[serde(
        default,
        rename = "albumIds",
        alias = "AlbumIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    album_ids: Vec<Uuid>,
    #[serde(default, rename = "minCommunityRating", alias = "MinCommunityRating")]
    min_community_rating: Option<f64>,
    #[serde(default, rename = "isMovie", alias = "IsMovie")]
    is_movie: Option<bool>,
    #[serde(default, rename = "isSeries", alias = "IsSeries")]
    is_series: Option<bool>,
    #[serde(default, rename = "isNews", alias = "IsNews")]
    is_news: Option<bool>,
    #[serde(default, rename = "isKids", alias = "IsKids")]
    is_kids: Option<bool>,
    #[serde(default, rename = "isSports", alias = "IsSports")]
    is_sports: Option<bool>,
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeItemTypes",
        alias = "ExcludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_types: Vec<String>,
    #[serde(
        default,
        rename = "mediaTypes",
        alias = "MediaTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
    #[serde(
        default,
        rename = "imageTypes",
        alias = "ImageTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    image_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeItemIds",
        alias = "ExcludeItemIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "videoTypes",
        alias = "VideoTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    video_types: Vec<String>,
    #[serde(default, rename = "isLocked", alias = "IsLocked")]
    is_locked: Option<bool>,
    #[serde(default, rename = "isPlaceHolder", alias = "IsPlaceHolder")]
    is_place_holder: Option<bool>,
    #[serde(default, rename = "hasOfficialRating", alias = "HasOfficialRating")]
    has_official_rating: Option<bool>,
    #[serde(default, rename = "collapseBoxSetItems", alias = "CollapseBoxSetItems")]
    collapse_box_set_items: Option<bool>,
    #[serde(default, rename = "minWidth", alias = "MinWidth")]
    min_width: Option<i32>,
    #[serde(default, rename = "minHeight", alias = "MinHeight")]
    min_height: Option<i32>,
    #[serde(default, rename = "maxWidth", alias = "MaxWidth")]
    max_width: Option<i32>,
    #[serde(default, rename = "maxHeight", alias = "MaxHeight")]
    max_height: Option<i32>,
    #[serde(default, rename = "is3D", alias = "Is3D")]
    is_3d: Option<bool>,
    #[serde(
        default,
        rename = "seriesStatus",
        alias = "SeriesStatus",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    series_status: Vec<String>,
    #[serde(
        default,
        rename = "nameStartsWithOrGreater",
        alias = "NameStartsWithOrGreater"
    )]
    name_starts_with_or_greater: Option<String>,
    #[serde(default, rename = "nameStartsWith", alias = "NameStartsWith")]
    name_starts_with: Option<String>,
    #[serde(default, rename = "nameLessThan", alias = "NameLessThan")]
    name_less_than: Option<String>,
    #[serde(
        default,
        rename = "studioIds",
        alias = "StudioIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    studio_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "genreIds",
        alias = "GenreIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    genre_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "audioLanguages",
        alias = "AudioLanguages",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    audio_languages: Vec<String>,
    #[serde(
        default,
        rename = "subtitleLanguages",
        alias = "SubtitleLanguages",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    subtitle_languages: Vec<String>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(
        default,
        rename = "sortBy",
        alias = "SortBy",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_by: Vec<String>,
    #[serde(
        default,
        rename = "sortOrder",
        alias = "SortOrder",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    sort_order: Vec<String>,
    #[serde(
        default = "default_total_record_count",
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount"
    )]
    enable_total_record_count: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemFilter {
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

impl FromStr for ItemFilter {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "isfolder" => Ok(Self::IsFolder),
            "isnotfolder" => Ok(Self::IsNotFolder),
            "isunplayed" => Ok(Self::IsUnplayed),
            "isplayed" => Ok(Self::IsPlayed),
            "isfavorite" => Ok(Self::IsFavorite),
            "isresumable" => Ok(Self::IsResumable),
            "likes" => Ok(Self::Likes),
            "dislikes" => Ok(Self::Dislikes),
            "isfavoriteorlikes" => Ok(Self::IsFavoriteOrLikes),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct LatestItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(default, rename = "isPlayed", alias = "IsPlayed")]
    is_played: Option<bool>,
    #[serde(default = "default_latest_limit")]
    limit: u64,
    #[serde(default, rename = "groupItems", alias = "GroupItems")]
    group_items: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SuggestionsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "mediaType",
        alias = "MediaType",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    media_types: Vec<String>,
    #[serde(
        default,
        rename = "type",
        alias = "Type",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    item_types: Vec<String>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(
        default,
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount"
    )]
    enable_total_record_count: bool,
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    query_items(state, headers, query).await
}

pub(crate) async fn get_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    get_for(state, headers, Some(user_id), query).await
}

pub(crate) async fn query_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: ItemsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    get_for(state, headers, query.user_id, query).await
}

pub(crate) async fn resume(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    resume_for(state, headers, query.user_id, query).await
}

pub(crate) async fn resume_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    resume_for(state, headers, Some(user_id), query).await
}

pub(crate) async fn latest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LatestItemsQuery>,
) -> Result<Json<Vec<user_library::BaseItemDto>>, ApiError> {
    latest_for(state, headers, query.user_id, query).await
}

pub(crate) async fn latest_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<LatestItemsQuery>,
) -> Result<Json<Vec<user_library::BaseItemDto>>, ApiError> {
    latest_for(state, headers, Some(user_id), query).await
}

pub(crate) async fn suggestions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SuggestionsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    suggestions_for(state, headers, query.user_id, query).await
}

pub(crate) async fn suggestions_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<SuggestionsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    suggestions_for(state, headers, Some(user_id), query).await
}

async fn get_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: ItemsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let mut query = query;
    let fields = std::mem::take(&mut query.fields);
    apply_items_controller_defaults(&state, &authenticated.user, target_user_id, &mut query)
        .await?;
    resolve_official_rating_filters(&state, &mut query).await?;

    if let Some(search_term) = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        let requested_start_index = query.start_index;
        let requested_limit = query.limit;
        let search_results = state
            .search
            .search_results(
                &authenticated.user,
                target_user_id,
                &SearchProviderQuery {
                    search_term,
                    include_item_types: &query.include_item_types,
                    exclude_item_types: &query.exclude_item_types,
                    media_types: &query.media_types,
                    parent_id: query.parent_id,
                    limit: requested_limit.map(|limit| limit.saturating_mul(3)),
                },
            )
            .await?;
        if !search_results.is_empty() {
            let scores = search_results
                .iter()
                .map(|result| (result.item_id, result.score))
                .collect::<HashMap<_, _>>();
            let mut ids = std::mem::take(&mut query.ids);
            for result in &search_results {
                if !ids.contains(&result.item_id) {
                    ids.push(result.item_id);
                }
            }
            query.ids = ids;
            query.search_term = None;
            query.start_index = 0;
            query.limit = None;

            let mut page = state
                .user_library
                .query_items(&authenticated.user, target_user_id, query.try_into()?)
                .await?;
            let total_record_count = page.items.len();
            page.items.sort_by(|left, right| {
                let left_score = scores.get(&left.id).copied().unwrap_or_default();
                let right_score = scores.get(&right.id).copied().unwrap_or_default();
                right_score
                    .total_cmp(&left_score)
                    .then_with(|| {
                        left.sort_name
                            .as_deref()
                            .or(left.name.as_deref())
                            .cmp(&right.sort_name.as_deref().or(right.name.as_deref()))
                    })
                    .then_with(|| left.id.cmp(&right.id))
            });
            let start = usize::try_from(requested_start_index).unwrap_or(usize::MAX);
            page.items = page
                .items
                .into_iter()
                .skip(start)
                .take(
                    requested_limit
                        .and_then(|limit| usize::try_from(limit).ok())
                        .unwrap_or(usize::MAX),
                )
                .collect();
            page.total_record_count = u64::try_from(total_record_count).unwrap_or(u64::MAX);
            page.start_index = requested_start_index;
            return Ok(Json(
                page_to_dto(state.as_ref(), page, fields, target_user_id).await?,
            ));
        }
    }

    let page = state
        .user_library
        .query_items(&authenticated.user, target_user_id, query.try_into()?)
        .await?;
    Ok(Json(
        page_to_dto(state.as_ref(), page, fields, target_user_id).await?,
    ))
}

async fn suggestions_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: SuggestionsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let enable_total_record_count = query.enable_total_record_count;
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                recursive: true,
                include_item_types: query.item_types,
                media_types: query.media_types,
                is_virtual_item: Some(false),
                order: BaseItemOrder::Random,
                start_index: query.start_index,
                limit: query.limit,
                enable_total_record_count: Some(enable_total_record_count),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    Ok(Json(
        page_to_dto(state.as_ref(), page, Vec::new(), target_user_id).await?,
    ))
}

async fn apply_items_controller_defaults(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    query: &mut ItemsQuery,
) -> Result<(), ApiError> {
    if query.parent_id == Some(Uuid::nil()) {
        query.parent_id = None;
    }
    let parent = match query.parent_id {
        Some(parent_id) => {
            state
                .user_library
                .item(authenticated_user, target_user_id, parent_id)
                .await?
        }
        None => {
            state
                .user_library
                .root(authenticated_user, target_user_id)
                .await?
        }
    };
    let collection_type = parent
        .data
        .as_ref()
        .and_then(|data| data.as_object())
        .and_then(|object| {
            object
                .get("CollectionType")
                .or_else(|| object.get("collection_type"))
        })
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let is_collection_folder = parent.item_type == "CollectionFolder";
    if collection_type.as_deref() == Some("playlists") {
        query.recursive = Some(true);
        query.include_item_types = vec!["Playlist".to_owned()];
    } else if is_collection_folder
        && query.include_item_types.is_empty()
        && collection_type.as_deref() == Some("boxsets")
    {
        query.include_item_types = vec!["BoxSet".to_owned()];
    }
    if is_collection_folder && !query.include_item_types.is_empty() && query.recursive.is_none() {
        query.recursive = Some(true);
    }
    Ok(())
}

async fn resolve_official_rating_filters(
    state: &AppState,
    query: &mut ItemsQuery,
) -> Result<(), ApiError> {
    if query.min_official_rating.is_none() && query.max_official_rating.is_none() {
        return Ok(());
    }
    let configuration = state.server_configuration.load().await?;
    let country = configuration.metadata_country_code;
    let min_score = query
        .min_official_rating
        .as_deref()
        .and_then(|rating| state.localization.rating_score(rating, &country, None));
    let max_score = query
        .max_official_rating
        .as_deref()
        .and_then(|rating| state.localization.rating_score(rating, &country, None));
    if min_score.is_none() && max_score.is_none() {
        return Ok(());
    }
    for rating in state.localization.parental_ratings(&country) {
        let Some(score) = rating.rating_score else {
            continue;
        };
        let passes_min = min_score.is_none_or(|minimum| {
            score.score > minimum.score
                || (score.score == minimum.score
                    && score.sub_score.unwrap_or(0) >= minimum.sub_score.unwrap_or(0))
        });
        let passes_max = max_score.is_none_or(|maximum| {
            score.score < maximum.score
                || (score.score == maximum.score
                    && score.sub_score.unwrap_or(0) <= maximum.sub_score.unwrap_or(0))
        });
        if passes_min
            && passes_max
            && !query
                .official_ratings
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&rating.name))
        {
            query.official_ratings.push(rating.name);
        }
    }
    Ok(())
}

async fn resume_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: ItemsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let mut query = query;
    let fields = std::mem::take(&mut query.fields);
    let page = state
        .user_library
        .resume_items(&authenticated.user, target_user_id, query.try_into()?)
        .await?;
    Ok(Json(
        page_to_dto(state.as_ref(), page, fields, target_user_id).await?,
    ))
}

async fn latest_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: LatestItemsQuery,
) -> Result<Json<Vec<user_library::BaseItemDto>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let target = state.users.get(target_user_id).await?;
    let configuration: UserConfiguration =
        serde_json::from_value(target.preferences).unwrap_or_default();
    let is_played = query.is_played.or({
        if configuration.hide_played_in_latest {
            Some(false)
        } else {
            None
        }
    });
    let mut query = query;
    let fields = std::mem::take(&mut query.fields);
    let _ = query.group_items;
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                parent_id: query.parent_id,
                recursive: true,
                include_item_types: query.include_item_types,
                is_virtual_item: Some(false),
                user_id: Some(target_user_id),
                is_played,
                order: BaseItemOrder::DateCreatedDescending,
                start_index: 0,
                limit: Some(query.limit),
                enable_total_record_count: Some(false),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    Ok(Json(
        page_to_dto(state.as_ref(), page, fields, target_user_id)
            .await?
            .items,
    ))
}

impl TryFrom<ItemsQuery> for BaseItemQuery {
    type Error = ApiError;

    #[allow(clippy::too_many_lines)]
    fn try_from(query: ItemsQuery) -> Result<Self, Self::Error> {
        let mut is_favorite = None;
        let mut is_resumable = None;
        let mut is_played = query.is_played;
        let mut is_folder = None;
        let mut is_liked = None;
        let mut is_favorite_or_liked = None;
        for filter in query.filters {
            match filter {
                ItemFilter::IsFolder => is_folder = Some(true),
                ItemFilter::IsNotFolder => is_folder = Some(false),
                ItemFilter::Likes => is_liked = Some(true),
                ItemFilter::Dislikes => is_liked = Some(false),
                ItemFilter::IsFavoriteOrLikes => is_favorite_or_liked = Some(true),
                ItemFilter::IsUnplayed => is_played = Some(false),
                ItemFilter::IsPlayed => is_played = Some(true),
                ItemFilter::IsFavorite => is_favorite = Some(true),
                ItemFilter::IsResumable => is_resumable = Some(true),
            }
        }
        if query.is_favorite.is_some() {
            is_favorite = query.is_favorite;
        }
        Ok(Self {
            ids: query.ids,
            exclude_ids: query.exclude_item_ids,
            genres: query.genres,
            studios: query.studios,
            artists: query.artists,
            albums: query.albums,
            years: query.years,
            tags: query.tags,
            person: query.person,
            person_ids: query.person_ids,
            person_types: query.person_types,
            min_community_rating: query.min_community_rating,
            min_critic_rating: query.min_critic_rating,
            is_favorite,
            is_folder,
            is_liked,
            is_favorite_or_liked,
            parent_id: query.parent_id,
            recursive: query.recursive.unwrap_or(false),
            search_term: query.search_term,
            include_item_types: query.include_item_types,
            exclude_item_types: query.exclude_item_types,
            media_types: query.media_types,
            image_types: query
                .image_types
                .iter()
                .filter_map(|name| image_type_code(name))
                .collect(),
            is_movie: query.is_movie,
            is_series: query.is_series,
            is_news: query.is_news,
            is_kids: query.is_kids,
            is_sports: query.is_sports,
            is_virtual_item: None,
            group_versions_by_presentation_key: false,
            user_id: query.user_id,
            is_resumable,
            is_played,
            min_premiere_date: query.min_premiere_date,
            max_premiere_date: query.max_premiere_date,
            min_date_last_saved: query.min_date_last_saved,
            min_date_last_saved_for_user: query.min_date_last_saved_for_user,
            has_overview: query.has_overview,
            has_official_rating: query.has_official_rating,
            has_parental_rating: query.has_parental_rating,
            has_imdb_id: query.has_imdb_id,
            has_tmdb_id: query.has_tmdb_id,
            has_tvdb_id: query.has_tvdb_id,
            has_subtitles: query.has_subtitles,
            has_theme_song: query.has_theme_song,
            has_theme_video: query.has_theme_video,
            has_special_feature: query.has_special_feature,
            has_trailer: query.has_trailer,
            is_hd: query.is_hd,
            is_4k: query.is_4k,
            min_width: query.min_width,
            max_width: query.max_width,
            min_height: query.min_height,
            max_height: query.max_height,
            is_3d: query.is_3d,
            is_locked: query.is_locked,
            is_placeholder: query.is_place_holder,
            is_missing: query.is_missing,
            is_unaired: query.is_unaired,
            index_number: query.index_number,
            parent_index_number: query.parent_index_number,
            adjacent_to: query.adjacent_to,
            location_types: query.location_types,
            exclude_location_types: query.exclude_location_types,
            video_types: query.video_types,
            series_statuses: query.series_status,
            official_ratings: query.official_ratings,
            audio_languages: query.audio_languages,
            subtitle_languages: query.subtitle_languages,
            studio_ids: query.studio_ids,
            genre_ids: query.genre_ids,
            artist_ids: query.artist_ids,
            exclude_artist_ids: query.exclude_artist_ids,
            album_artist_ids: query.album_artist_ids,
            contributing_artist_ids: query.contributing_artist_ids,
            album_ids: query.album_ids,
            name_starts_with_or_greater: query.name_starts_with_or_greater,
            name_starts_with: query.name_starts_with,
            name_less_than: query.name_less_than,
            collapse_box_set_items: query.collapse_box_set_items.unwrap_or(false),
            allowed_official_ratings: Vec::new(),
            allowed_parental_ratings: Vec::new(),
            block_unrated_items: Vec::new(),
            blocked_tags: Vec::new(),
            allowed_tags: Vec::new(),
            enabled_folders: Vec::new(),
            enable_all_folders: true,
            blocked_media_folders: None,
            order: item_order(&query.sort_by, &query.sort_order),
            start_index: query.start_index,
            limit: query.limit,
            enable_total_record_count: Some(query.enable_total_record_count),
        })
    }
}

impl ItemsQuery {
    pub(crate) fn force_include_item_type(&mut self, item_type: impl Into<String>) {
        self.include_item_types = vec![item_type.into()];
    }
}

pub(crate) fn item_order(sort_by: &[String], sort_order: &[String]) -> BaseItemOrder {
    let requested_sort_order: Vec<_> = sort_order
        .first()
        .and_then(|order| crate::query::parse_sort_order(order).ok())
        .into_iter()
        .collect();
    let order_by = crate::query::get_order_by(sort_by, &requested_sort_order);
    let descending = order_by
        .first()
        .is_some_and(|(_, order)| *order == SortOrder::Descending);

    let Some((sort, _)) = order_by.first() else {
        return BaseItemOrder::default();
    };
    if sort.eq_ignore_ascii_case("Random") {
        return BaseItemOrder::Random;
    }
    let order = if descending {
        SortOrder::Descending
    } else {
        SortOrder::Ascending
    };
    let known_sort = match sort.as_str() {
        sort if sort.eq_ignore_ascii_case("DateCreated") => BaseItemOrder::DateCreatedAscending,
        sort if sort.eq_ignore_ascii_case("DatePlayed") => BaseItemOrder::DatePlayedAscending,
        sort if sort.eq_ignore_ascii_case("PremiereDate") => BaseItemOrder::PremiereDateAscending,
        sort if sort.eq_ignore_ascii_case("PlayCount") => BaseItemOrder::PlayCountAscending,
        sort if sort.eq_ignore_ascii_case("CommunityRating") => {
            BaseItemOrder::CommunityRatingAscending
        }
        sort if sort.eq_ignore_ascii_case("CriticRating") => BaseItemOrder::CriticRatingAscending,
        sort if sort.eq_ignore_ascii_case("Runtime") => BaseItemOrder::RuntimeTicksAscending,
        sort if sort.eq_ignore_ascii_case("AiredEpisodeOrder") => {
            BaseItemOrder::AiredEpisodeOrderAscending
        }
        sort if sort.eq_ignore_ascii_case("Album") => BaseItemOrder::AlbumAscending,
        sort if sort.eq_ignore_ascii_case("AlbumArtist") => BaseItemOrder::AlbumArtistAscending,
        sort if sort.eq_ignore_ascii_case("Artist") => BaseItemOrder::ArtistAscending,
        sort if sort.eq_ignore_ascii_case("OfficialRating") => {
            BaseItemOrder::OfficialRatingAscending
        }
        sort if sort.eq_ignore_ascii_case("StartDate") => BaseItemOrder::StartDateAscending,
        sort if sort.eq_ignore_ascii_case("IsFolder") => BaseItemOrder::IsFolderAscending,
        sort if sort.eq_ignore_ascii_case("IsUnplayed") => BaseItemOrder::IsUnplayedAscending,
        sort if sort.eq_ignore_ascii_case("IsPlayed") => BaseItemOrder::IsPlayedAscending,
        sort if sort.eq_ignore_ascii_case("SeriesSortName") => {
            BaseItemOrder::SeriesSortNameAscending
        }
        sort if sort.eq_ignore_ascii_case("VideoBitRate") => BaseItemOrder::VideoBitRateAscending,
        sort if sort.eq_ignore_ascii_case("AirTime") => BaseItemOrder::AirTimeAscending,
        sort if sort.eq_ignore_ascii_case("Studio") => BaseItemOrder::StudioAscending,
        sort if sort.eq_ignore_ascii_case("IsFavoriteOrLiked") => {
            BaseItemOrder::IsFavoriteOrLikedAscending
        }
        sort if sort.eq_ignore_ascii_case("DateLastContentAdded") => {
            BaseItemOrder::DateLastContentAddedAscending
        }
        sort if sort.eq_ignore_ascii_case("ParentIndexNumber") => {
            BaseItemOrder::ParentIndexNumberAscending
        }
        sort if sort.eq_ignore_ascii_case("IndexNumber") => BaseItemOrder::IndexNumberAscending,
        sort if sort.eq_ignore_ascii_case("SortName") || sort.eq_ignore_ascii_case("Name") => {
            BaseItemOrder::SortName
        }
        _ => return BaseItemOrder::default(),
    };
    if order == SortOrder::Descending {
        known_sort.descending()
    } else {
        known_sort
    }
}

fn image_type_code(name: &str) -> Option<i16> {
    match name.to_ascii_lowercase().as_str() {
        "primary" => Some(0),
        "art" => Some(1),
        "backdrop" => Some(2),
        "banner" => Some(3),
        "logo" => Some(4),
        "thumb" => Some(5),
        "disc" => Some(6),
        "box" => Some(7),
        "screenshot" => Some(8),
        "menu" => Some(9),
        "chapter" => Some(10),
        "boxrear" => Some(11),
        "profile" => Some(12),
        _ => None,
    }
}

const fn default_latest_limit() -> u64 {
    20
}

const fn default_total_record_count() -> bool {
    true
}

async fn page_to_dto(
    state: &AppState,
    page: BaseItemPage,
    fields: Vec<String>,
    target_user_id: Uuid,
) -> Result<user_library::BaseItemQueryResult, ApiError> {
    let requested_fields = user_library::BaseItemDtoFields::from_names(&fields);
    let defaults =
        user_library::media_stream_defaults_for_user(state, target_user_id, requested_fields)
            .await?;
    let mut remembered_user_data = if requested_fields.wants_media_streams() {
        state
            .user_data
            .get_preferred_for_items(target_user_id, &page.items)
            .await?
    } else {
        std::collections::HashMap::new()
    };
    let mut media_streams = if requested_fields.wants_media_streams() {
        let item_ids = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
        state
            .media_streams
            .get_media_streams_for_items(&item_ids)
            .await?
    } else {
        std::collections::HashMap::new()
    };
    let mut media_attachments = if requested_fields.wants_media_attachments() {
        let item_ids = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
        state
            .media_attachments
            .get_media_attachments_for_items(&item_ids)
            .await?
    } else {
        std::collections::HashMap::new()
    };
    let mut trickplay_manifests =
        user_library::trickplay_manifests_for_items(state, &page.items, requested_fields).await?;
    let mut user_dtos = state
        .user_data
        .preferred_dto_map(target_user_id, &page.items)
        .await?;
    let mut relations = user_library::load_relation_metadata(state, &page.items).await?;

    let mut items = Vec::with_capacity(page.items.len());
    for item in page.items {
        let item_id = item.id;
        let original_language = user_library::original_language_from_item(&item);
        let mut dto = user_library::item_to_dto(item, state.server_id());
        if let Some(user_data) = user_dtos.remove(&item_id) {
            user_library::attach_user_data_dto(&mut dto, user_data);
        }
        if let Some(metadata) = relations.remove(&item_id) {
            user_library::attach_relation_metadata(&mut dto, metadata);
        }
        if let Some(projection) = state
            .dto_images
            .project(
                item_id,
                jellyfin_server_implementations::DtoImageOptions::default(),
            )
            .await
            .map_err(|_| ApiError::Internal)?
        {
            user_library::attach_dto_image_projection(&mut dto, projection);
        }
        if requested_fields.wants_media_streams() {
            let streams = media_streams.remove(&item_id).unwrap_or_default();
            let attachments = media_attachments.remove(&item_id).unwrap_or_default();
            let remembered = remembered_user_data.remove(&item_id);
            user_library::project_item_dto_with_streams(
                &mut dto,
                requested_fields,
                streams,
                attachments,
                defaults.as_ref(),
                remembered.as_ref(),
                original_language.as_deref(),
            );
        }
        user_library::attach_trickplay_manifest(
            &mut dto,
            requested_fields,
            trickplay_manifests.remove(&item_id).unwrap_or_default(),
        );
        items.push(dto);
    }

    Ok(user_library::BaseItemQueryResult {
        items,
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_parse_official_names_case_insensitively() {
        assert_eq!(
            "IsFavorite".parse::<ItemFilter>(),
            Ok(ItemFilter::IsFavorite)
        );
        assert_eq!("IsPlayed".parse::<ItemFilter>(), Ok(ItemFilter::IsPlayed));
        assert_eq!(
            "isunplayed".parse::<ItemFilter>(),
            Ok(ItemFilter::IsUnplayed)
        );
        assert_eq!("likes".parse::<ItemFilter>(), Ok(ItemFilter::Likes));
        assert!("UnknownFilter".parse::<ItemFilter>().is_err());
    }

    #[test]
    fn item_order_maps_official_extended_sort_fields() {
        assert_eq!(
            item_order(&["PlayCount".to_owned()], &[]),
            BaseItemOrder::PlayCountAscending
        );
        assert_eq!(
            item_order(&["PlayCount".to_owned()], &["Descending".to_owned()]),
            BaseItemOrder::PlayCountDescending
        );
        assert_eq!(
            item_order(&["CommunityRating".to_owned()], &[]),
            BaseItemOrder::CommunityRatingAscending
        );
        assert_eq!(
            item_order(&["CriticRating".to_owned()], &["Descending".to_owned()]),
            BaseItemOrder::CriticRatingDescending
        );
        assert_eq!(
            item_order(&["Runtime".to_owned()], &[]),
            BaseItemOrder::RuntimeTicksAscending
        );
        assert_eq!(
            item_order(&["Runtime".to_owned()], &["Descending".to_owned()]),
            BaseItemOrder::RuntimeTicksDescending
        );
    }

    #[test]
    fn item_order_keeps_premiere_direction() {
        assert_eq!(
            item_order(&["PremiereDate".to_owned()], &["Descending".to_owned()]),
            BaseItemOrder::PremiereDateDescending
        );
    }

    #[test]
    fn item_order_maps_all_requested_official_fields() {
        let ascending_fields = [
            (
                "AiredEpisodeOrder",
                BaseItemOrder::AiredEpisodeOrderAscending,
            ),
            ("Album", BaseItemOrder::AlbumAscending),
            ("AlbumArtist", BaseItemOrder::AlbumArtistAscending),
            ("Artist", BaseItemOrder::ArtistAscending),
            ("OfficialRating", BaseItemOrder::OfficialRatingAscending),
            ("StartDate", BaseItemOrder::StartDateAscending),
            ("IsFolder", BaseItemOrder::IsFolderAscending),
            ("IsUnplayed", BaseItemOrder::IsUnplayedAscending),
            ("IsPlayed", BaseItemOrder::IsPlayedAscending),
            ("SeriesSortName", BaseItemOrder::SeriesSortNameAscending),
            ("VideoBitRate", BaseItemOrder::VideoBitRateAscending),
            ("AirTime", BaseItemOrder::AirTimeAscending),
            ("Studio", BaseItemOrder::StudioAscending),
            (
                "IsFavoriteOrLiked",
                BaseItemOrder::IsFavoriteOrLikedAscending,
            ),
            (
                "DateLastContentAdded",
                BaseItemOrder::DateLastContentAddedAscending,
            ),
            (
                "ParentIndexNumber",
                BaseItemOrder::ParentIndexNumberAscending,
            ),
            ("IndexNumber", BaseItemOrder::IndexNumberAscending),
        ];
        for (field, ascending) in ascending_fields {
            assert_eq!(
                item_order(&[field.to_owned()], &[]),
                ascending,
                "{field} should map with the requested order"
            );
            assert_eq!(
                item_order(&[field.to_owned()], &["Descending".to_owned()]),
                ascending.descending(),
                "{field} should preserve Descending"
            );
        }
        assert_eq!(
            item_order(&["UnknownSortField".to_owned()], &[]),
            BaseItemOrder::SortName
        );
    }
}
