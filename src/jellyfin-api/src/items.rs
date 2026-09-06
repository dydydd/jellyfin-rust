use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use chrono::{DateTime, Utc};
use jellyfin_controller::SearchProviderQuery;
use jellyfin_data::{BaseItemOrder, BaseItemPage, BaseItemQuery, entities::base_item};
use jellyfin_model::{SortOrder, UserConfiguration};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: u64,
    #[serde(default, alias = "Limit")]
    limit: Option<u64>,
    #[serde(default, alias = "Recursive")]
    recursive: Option<bool>,
    #[serde(rename = "searchTerm", alias = "SearchTerm", alias = "searchterm")]
    search_term: Option<String>,
    #[serde(rename = "parentId", alias = "ParentId", alias = "parentid")]
    parent_id: Option<Uuid>,
    #[serde(default, rename = "isPlayed", alias = "IsPlayed", alias = "isplayed")]
    is_played: Option<bool>,
    #[serde(
        default,
        rename = "isFavorite",
        alias = "IsFavorite",
        alias = "isfavorite"
    )]
    is_favorite: Option<bool>,
    #[serde(
        default,
        rename = "minOfficialRating",
        alias = "MinOfficialRating",
        alias = "minofficialrating"
    )]
    min_official_rating: Option<String>,
    #[serde(
        default,
        rename = "maxOfficialRating",
        alias = "MaxOfficialRating",
        alias = "maxofficialrating"
    )]
    max_official_rating: Option<String>,
    #[serde(
        default,
        rename = "hasThemeSong",
        alias = "HasThemeSong",
        alias = "hasthemesong"
    )]
    has_theme_song: Option<bool>,
    #[serde(
        default,
        rename = "hasThemeVideo",
        alias = "HasThemeVideo",
        alias = "hasthemevideo"
    )]
    has_theme_video: Option<bool>,
    #[serde(
        default,
        rename = "hasSubtitles",
        alias = "HasSubtitles",
        alias = "hassubtitles"
    )]
    has_subtitles: Option<bool>,
    #[serde(
        default,
        rename = "hasSpecialFeature",
        alias = "HasSpecialFeature",
        alias = "hasspecialfeature"
    )]
    has_special_feature: Option<bool>,
    #[serde(
        default,
        rename = "hasTrailer",
        alias = "HasTrailer",
        alias = "hastrailer"
    )]
    has_trailer: Option<bool>,
    #[serde(
        default,
        rename = "adjacentTo",
        alias = "AdjacentTo",
        alias = "adjacentto"
    )]
    adjacent_to: Option<Uuid>,
    #[serde(
        default,
        rename = "indexNumber",
        alias = "IndexNumber",
        alias = "indexnumber"
    )]
    index_number: Option<i32>,
    #[serde(
        default,
        rename = "parentIndexNumber",
        alias = "ParentIndexNumber",
        alias = "parentindexnumber"
    )]
    parent_index_number: Option<i32>,
    #[serde(
        default,
        rename = "hasParentalRating",
        alias = "HasParentalRating",
        alias = "hasparentalrating"
    )]
    has_parental_rating: Option<bool>,
    #[serde(default, rename = "isHd", alias = "IsHD", alias = "ishd")]
    is_hd: Option<bool>,
    #[serde(default, rename = "is4K", alias = "Is4K", alias = "is4k")]
    is_4k: Option<bool>,
    #[serde(
        default,
        rename = "locationTypes",
        alias = "LocationTypes",
        alias = "locationtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    location_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeLocationTypes",
        alias = "ExcludeLocationTypes",
        alias = "excludelocationtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_location_types: Vec<String>,
    #[serde(
        default,
        rename = "isMissing",
        alias = "IsMissing",
        alias = "ismissing"
    )]
    is_missing: Option<bool>,
    #[serde(
        default,
        rename = "isUnaired",
        alias = "IsUnaired",
        alias = "isunaired"
    )]
    is_unaired: Option<bool>,
    #[serde(
        default,
        rename = "minCriticRating",
        alias = "MinCriticRating",
        alias = "mincriticrating"
    )]
    min_critic_rating: Option<f64>,
    #[serde(
        default,
        rename = "minPremiereDate",
        alias = "MinPremiereDate",
        alias = "minpremieredate"
    )]
    min_premiere_date: Option<DateTime<Utc>>,
    #[serde(
        default,
        rename = "maxPremiereDate",
        alias = "MaxPremiereDate",
        alias = "maxpremieredate"
    )]
    max_premiere_date: Option<DateTime<Utc>>,
    #[serde(
        default,
        rename = "minDateLastSaved",
        alias = "MinDateLastSaved",
        alias = "mindatelastsaved"
    )]
    min_date_last_saved: Option<DateTime<Utc>>,
    #[serde(
        default,
        rename = "minDateLastSavedForUser",
        alias = "MinDateLastSavedForUser",
        alias = "mindatelastsavedforuser"
    )]
    min_date_last_saved_for_user: Option<DateTime<Utc>>,
    #[serde(
        default,
        rename = "hasOverview",
        alias = "HasOverview",
        alias = "hasoverview"
    )]
    has_overview: Option<bool>,
    #[serde(
        default,
        rename = "hasImdbId",
        alias = "HasImdbId",
        alias = "hasimdbid"
    )]
    has_imdb_id: Option<bool>,
    #[serde(
        default,
        rename = "hasTmdbId",
        alias = "HasTmdbId",
        alias = "hastmdbid"
    )]
    has_tmdb_id: Option<bool>,
    #[serde(
        default,
        rename = "hasTvdbId",
        alias = "HasTvdbId",
        alias = "hastvdbid"
    )]
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
        alias = "personids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    person_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "personTypes",
        alias = "PersonTypes",
        alias = "persontypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    person_types: Vec<String>,
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
        alias = "excludeartistids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "artistIds",
        alias = "ArtistIds",
        alias = "artistids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "albumArtistIds",
        alias = "AlbumArtistIds",
        alias = "albumartistids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    album_artist_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "contributingArtistIds",
        alias = "ContributingArtistIds",
        alias = "contributingartistids",
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
        alias = "albumids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    album_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "minCommunityRating",
        alias = "MinCommunityRating",
        alias = "mincommunityrating"
    )]
    min_community_rating: Option<f64>,
    #[serde(default, rename = "isMovie", alias = "IsMovie", alias = "ismovie")]
    is_movie: Option<bool>,
    #[serde(default, rename = "isSeries", alias = "IsSeries", alias = "isseries")]
    is_series: Option<bool>,
    #[serde(default, rename = "isNews", alias = "IsNews", alias = "isnews")]
    is_news: Option<bool>,
    #[serde(default, rename = "isKids", alias = "IsKids", alias = "iskids")]
    is_kids: Option<bool>,
    #[serde(default, rename = "isSports", alias = "IsSports", alias = "issports")]
    is_sports: Option<bool>,
    #[serde(
        default,
        alias = "Ids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<Uuid>,
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
        rename = "imageTypes",
        alias = "ImageTypes",
        alias = "imagetypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    image_types: Vec<String>,
    #[serde(
        default,
        rename = "excludeItemIds",
        alias = "ExcludeItemIds",
        alias = "excludeitemids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_ids: Vec<Uuid>,
    #[serde(
        default,
        rename = "videoTypes",
        alias = "VideoTypes",
        alias = "videotypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    video_types: Vec<String>,
    #[serde(default, rename = "isLocked", alias = "IsLocked", alias = "islocked")]
    is_locked: Option<bool>,
    #[serde(
        default,
        rename = "isPlaceHolder",
        alias = "IsPlaceHolder",
        alias = "isplaceholder"
    )]
    is_place_holder: Option<bool>,
    #[serde(
        default,
        rename = "hasOfficialRating",
        alias = "HasOfficialRating",
        alias = "hasofficialrating"
    )]
    has_official_rating: Option<bool>,
    #[serde(
        default,
        rename = "collapseBoxSetItems",
        alias = "CollapseBoxSetItems",
        alias = "collapseboxsetitems"
    )]
    collapse_box_set_items: Option<bool>,
    #[serde(default, rename = "minWidth", alias = "MinWidth", alias = "minwidth")]
    min_width: Option<i32>,
    #[serde(
        default,
        rename = "minHeight",
        alias = "MinHeight",
        alias = "minheight"
    )]
    min_height: Option<i32>,
    #[serde(default, rename = "maxWidth", alias = "MaxWidth", alias = "maxwidth")]
    max_width: Option<i32>,
    #[serde(
        default,
        rename = "maxHeight",
        alias = "MaxHeight",
        alias = "maxheight"
    )]
    max_height: Option<i32>,
    #[serde(default, rename = "is3D", alias = "Is3D", alias = "is3d")]
    is_3d: Option<bool>,
    #[serde(
        default,
        rename = "seriesStatus",
        alias = "SeriesStatus",
        alias = "seriesstatus",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    series_status: Vec<String>,
    #[serde(
        default,
        rename = "nameStartsWithOrGreater",
        alias = "NameStartsWithOrGreater",
        alias = "namestartswithorgreater"
    )]
    name_starts_with_or_greater: Option<String>,
    #[serde(
        default,
        rename = "nameStartsWith",
        alias = "NameStartsWith",
        alias = "namestartswith"
    )]
    name_starts_with: Option<String>,
    #[serde(
        default,
        rename = "nameLessThan",
        alias = "NameLessThan",
        alias = "namelessthan"
    )]
    name_less_than: Option<String>,
    #[serde(
        default,
        rename = "studioIds",
        alias = "StudioIds",
        alias = "studioids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    studio_ids: Vec<Uuid>,
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
        rename = "audioLanguages",
        alias = "AudioLanguages",
        alias = "audiolanguages",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    audio_languages: Vec<String>,
    #[serde(
        default,
        rename = "subtitleLanguages",
        alias = "SubtitleLanguages",
        alias = "subtitlelanguages",
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
    #[serde(
        default = "default_total_record_count",
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount",
        alias = "enabletotalrecordcount"
    )]
    enable_total_record_count: bool,
    #[serde(
        default,
        rename = "excludeActiveSessions",
        alias = "ExcludeActiveSessions",
        alias = "excludeactivesessions",
        alias = "exclude_active_sessions"
    )]
    exclude_active_sessions: bool,
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
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
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
        default,
        rename = "includeItemTypes",
        alias = "IncludeItemTypes",
        alias = "includeitemtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_item_types: Vec<String>,
    #[serde(default, rename = "isPlayed", alias = "IsPlayed", alias = "isplayed")]
    is_played: Option<bool>,
    #[serde(
        rename = "enableImages",
        alias = "EnableImages",
        alias = "enableimages"
    )]
    enable_images: Option<bool>,
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
        rename = "enableUserData",
        alias = "EnableUserData",
        alias = "enableuserdata"
    )]
    enable_user_data: Option<bool>,
    #[serde(default = "default_latest_limit", alias = "Limit")]
    limit: u64,
    #[serde(
        default = "default_true",
        rename = "groupItems",
        alias = "GroupItems",
        alias = "groupitems"
    )]
    group_items: bool,
}

const SUGGESTION_MEDIA_TYPES: &[&str] = &["Unknown", "Video", "Audio", "Photo", "Book"];

const SUGGESTION_ITEM_TYPES: &[&str] = &[
    "AggregateFolder",
    "Audio",
    "AudioBook",
    "BasePluginFolder",
    "Book",
    "BoxSet",
    "Channel",
    "ChannelFolderItem",
    "CollectionFolder",
    "Episode",
    "Folder",
    "Genre",
    "ManualPlaylistsFolder",
    "Movie",
    "LiveTvChannel",
    "LiveTvProgram",
    "MusicAlbum",
    "MusicArtist",
    "MusicGenre",
    "MusicVideo",
    "Person",
    "Photo",
    "PhotoAlbum",
    "Playlist",
    "PlaylistsFolder",
    "Program",
    "Recording",
    "Season",
    "Series",
    "Studio",
    "Trailer",
    "TvChannel",
    "TvProgram",
    "UserRootFolder",
    "UserView",
    "Video",
    "Year",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SuggestionMediaType(&'static str);

impl SuggestionMediaType {
    const fn as_str(self) -> &'static str {
        self.0
    }
}

impl FromStr for SuggestionMediaType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_suggestion_enum(value, SUGGESTION_MEDIA_TYPES).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SuggestionItemType(&'static str);

impl SuggestionItemType {
    const fn as_str(self) -> &'static str {
        self.0
    }
}

impl FromStr for SuggestionItemType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_suggestion_enum(value, SUGGESTION_ITEM_TYPES).map(Self)
    }
}

fn parse_suggestion_enum(
    value: &str,
    variants: &'static [&'static str],
) -> Result<&'static str, ()> {
    let value = value.trim();
    if let Ok(index) = value.parse::<i32>() {
        return usize::try_from(index)
            .ok()
            .and_then(|index| variants.get(index).copied())
            .ok_or(());
    }
    variants
        .iter()
        .copied()
        .find(|variant| variant.eq_ignore_ascii_case(value))
        .ok_or(())
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SuggestionsQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "mediaType",
        alias = "MediaType",
        alias = "mediatype",
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    media_types: Vec<SuggestionMediaType>,
    #[serde(
        default,
        rename = "type",
        alias = "Type",
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    item_types: Vec<SuggestionItemType>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(default, alias = "Limit")]
    limit: Option<i32>,
    #[serde(
        default,
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount",
        alias = "enabletotalrecordcount"
    )]
    enable_total_record_count: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SuggestionsResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: usize,
    start_index: i32,
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
) -> Result<Json<SuggestionsResult>, ApiError> {
    suggestions_for(state, headers, query.user_id, query).await
}

pub(crate) async fn suggestions_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<SuggestionsQuery>,
) -> Result<Json<SuggestionsResult>, ApiError> {
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
    let parent_scope = resolve_user_view_parent_scope(
        &state,
        &authenticated.user,
        target_user_id,
        query.parent_id,
    )
    .await?;
    query.parent_id = parent_scope.parent_id;
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

            let mut database_query: BaseItemQuery = query.try_into()?;
            database_query.parent_ids = parent_scope.parent_ids;
            let mut page = state
                .user_library
                .query_items(&authenticated.user, target_user_id, database_query)
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

    let mut database_query: BaseItemQuery = query.try_into()?;
    database_query.parent_ids = parent_scope.parent_ids;
    let page = state
        .user_library
        .query_items(&authenticated.user, target_user_id, database_query)
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
) -> Result<Json<SuggestionsResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let requested_start_index = query.start_index.unwrap_or_default();
    let enable_total_record_count = query.enable_total_record_count;
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                recursive: true,
                include_item_types: query
                    .item_types
                    .into_iter()
                    .map(|item_type| item_type.as_str().to_owned())
                    .collect(),
                media_types: query
                    .media_types
                    .into_iter()
                    .map(|media_type| media_type.as_str().to_owned())
                    .collect(),
                is_virtual_item: Some(false),
                order: BaseItemOrder::Random,
                start_index: u64::try_from(requested_start_index).unwrap_or_default(),
                limit: query
                    .limit
                    .filter(|limit| *limit >= 0)
                    .map(|limit| u64::try_from(limit).unwrap_or_default()),
                enable_total_record_count: Some(enable_total_record_count),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let result = page_to_dto(state.as_ref(), page, Vec::new(), target_user_id).await?;
    Ok(Json(SuggestionsResult {
        items: result.items,
        total_record_count: result.total_record_count,
        start_index: requested_start_index,
    }))
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
    let parent_scope = resolve_user_view_parent_scope(
        &state,
        &authenticated.user,
        target_user_id,
        query.parent_id,
    )
    .await?;
    query.parent_id = parent_scope.parent_id;
    let exclude_active_sessions = query.exclude_active_sessions;
    let mut database_query: BaseItemQuery = query.try_into()?;
    database_query.parent_ids = parent_scope.parent_ids;
    if exclude_active_sessions {
        let active_item_ids = state
            .devices
            .active_now_playing_item_ids(target_user_id)
            .await?;
        if !active_item_ids.is_empty() {
            // Official Jellyfin records the displayed primary in
            // NowPlayingItem, while Resume can surface the alternate version
            // that owns the latest progress. Expand all active video groups in
            // one batch and feed the ids into the shared PostgreSQL candidate
            // filter so page rows and TotalRecordCount stay consistent.
            let active_versions = state
                .base_items
                .media_source_versions_for_items(&active_item_ids)
                .await?;
            database_query.exclude_ids.extend(active_item_ids);
            database_query
                .exclude_ids
                .extend(active_versions.into_iter().map(|version| version.id));
            database_query.exclude_ids.sort_unstable();
            database_query.exclude_ids.dedup();
        }
    }
    let page = state
        .user_library
        .resume_items(&authenticated.user, target_user_id, database_query)
        .await?;
    Ok(Json(
        page_to_dto(state.as_ref(), page, fields, target_user_id).await?,
    ))
}

#[allow(clippy::too_many_lines)]
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
    let enable_image_types = parse_image_type_selectors(&query.enable_image_types);
    let dto_options = PageDtoOptions {
        enable_images: query.enable_images.unwrap_or(true),
        image_type_limit: query
            .image_type_limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0)),
        enable_image_types,
        enable_user_data: query.enable_user_data.unwrap_or(true),
    };
    let parent_scope = resolve_user_view_parent_scope(
        &state,
        &authenticated.user,
        target_user_id,
        query.parent_id,
    )
    .await?;
    let include_item_types = std::mem::take(&mut query.include_item_types);
    let is_folder = include_item_types.is_empty().then_some(false);
    let exclude_item_types = include_item_types.is_empty().then(|| {
        ["Person", "Studio", "Year", "MusicGenre", "Genre"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    });
    let candidate_limit = if query.group_items {
        query.limit.saturating_mul(2)
    } else {
        query.limit
    };
    let database_query = BaseItemQuery {
        parent_id: parent_scope.parent_id,
        parent_ids: parent_scope.parent_ids,
        recursive: true,
        include_item_types,
        exclude_item_types: exclude_item_types.unwrap_or_default(),
        is_folder,
        is_virtual_item: Some(false),
        user_id: Some(target_user_id),
        is_played,
        order: BaseItemOrder::DateCreatedDescending,
        start_index: 0,
        limit: Some(candidate_limit),
        enable_total_record_count: Some(false),
        ..BaseItemQuery::default()
    };
    let grouping_query = database_query.clone();
    let mut page = state
        .user_library
        .query_items(&authenticated.user, target_user_id, database_query)
        .await?;
    // The official latest-media repository uses the identifier as its stable
    // descending tie-breaker after DateCreated. Keep that order before
    // application-side container grouping.
    page.items.sort_by(|left, right| {
        right
            .date_created
            .cmp(&left.date_created)
            .then_with(|| right.id.cmp(&left.id))
    });
    let selections = if query.group_items {
        group_latest_items(
            state.as_ref(),
            &authenticated.user,
            target_user_id,
            page.items,
            query.limit,
            grouping_query,
        )
        .await?
    } else {
        page.items
            .into_iter()
            .take(usize::try_from(query.limit).unwrap_or(usize::MAX))
            .map(|item| LatestItemSelection {
                item,
                child_count: None,
            })
            .collect()
    };
    let child_counts = selections
        .iter()
        .map(|selection| selection.child_count)
        .collect::<Vec<_>>();
    let page = BaseItemPage {
        total_record_count: u64::try_from(selections.len()).unwrap_or(u64::MAX),
        start_index: 0,
        items: selections
            .into_iter()
            .map(|selection| selection.item)
            .collect(),
    };
    let mut items =
        page_to_dto_with_options(state.as_ref(), page, fields, target_user_id, &dto_options)
            .await?
            .items;
    for (item, child_count) in items.iter_mut().zip(child_counts) {
        if child_count.is_some() {
            item.child_count = child_count;
        }
    }
    Ok(Json(items))
}

#[derive(Debug)]
struct LatestItemSelection {
    item: base_item::Model,
    child_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LatestItemGroupKey {
    Container(Uuid),
    Presentation(String),
}

#[derive(Debug)]
enum LatestGroupingCandidate {
    Item(base_item::Model),
    Tv {
        selection: LatestItemSelection,
        max_date: DateTime<Utc>,
    },
}

async fn group_latest_items(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    candidates: Vec<base_item::Model>,
    limit: u64,
    grouping_query: BaseItemQuery,
) -> Result<Vec<LatestItemSelection>, ApiError> {
    let tv_groups = state
        .user_library
        .latest_tv_groups(authenticated_user, target_user_id, grouping_query, limit)
        .await?;
    let audio_ids = candidates
        .iter()
        .filter(|item| item.item_type.eq_ignore_ascii_case("Audio"))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let photo_ids = candidates
        .iter()
        .filter(|item| item.item_type.eq_ignore_ascii_case("Photo"))
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let music_album_types = ["MusicAlbum".to_owned()];
    let photo_album_types = ["PhotoAlbum".to_owned()];
    let (audio_album_ids, photo_album_ids) = tokio::try_join!(
        state
            .base_items
            .nearest_ancestor_ids_by_type(&audio_ids, &music_album_types),
        state
            .base_items
            .nearest_ancestor_ids_by_type(&photo_ids, &photo_album_types),
    )?;
    let mut container_ids = tv_groups
        .iter()
        .filter_map(|group| group.selected_container_id)
        .collect::<Vec<_>>();
    container_ids.extend(tv_groups.iter().map(|group| group.most_recent_episode_id));
    container_ids.extend(audio_album_ids.values().copied());
    container_ids.extend(photo_album_ids.values().copied());
    container_ids.sort_unstable();
    container_ids.dedup();
    let grouping_items = if container_ids.is_empty() {
        HashMap::new()
    } else {
        state
            .user_library
            .query_items(
                authenticated_user,
                target_user_id,
                BaseItemQuery {
                    ids: container_ids,
                    user_id: Some(target_user_id),
                    enable_total_record_count: Some(false),
                    ..BaseItemQuery::default()
                },
            )
            .await?
            .items
            .into_iter()
            .map(|item| (item.id, item))
            .collect::<HashMap<_, _>>()
    };

    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut candidates = candidates
        .into_iter()
        .filter(|item| !item.item_type.eq_ignore_ascii_case("Episode") || item.series_id.is_none())
        .map(LatestGroupingCandidate::Item)
        .collect::<Vec<_>>();
    for tv_group in tv_groups {
        let Some(most_recent_episode) = grouping_items.get(&tv_group.most_recent_episode_id) else {
            continue;
        };
        let container = tv_group
            .selected_container_id
            .and_then(|id| grouping_items.get(&id))
            .filter(|container| {
                container.item_type.eq_ignore_ascii_case("Season")
                    || container.item_type.eq_ignore_ascii_case("Series")
            });
        candidates.push(LatestGroupingCandidate::Tv {
            selection: LatestItemSelection {
                item: container.unwrap_or(most_recent_episode).clone(),
                child_count: container
                    .map(|_| u64::try_from(tv_group.recent_child_count).unwrap_or_default()),
            },
            max_date: tv_group.max_date,
        });
    }
    candidates.sort_by(|left, right| {
        let (left_date, left_id) = match left {
            LatestGroupingCandidate::Item(item) => (item.date_created, item.id),
            LatestGroupingCandidate::Tv {
                selection,
                max_date,
            } => (*max_date, selection.item.id),
        };
        let (right_date, right_id) = match right {
            LatestGroupingCandidate::Item(item) => (item.date_created, item.id),
            LatestGroupingCandidate::Tv {
                selection,
                max_date,
            } => (*max_date, selection.item.id),
        };
        right_date
            .cmp(&left_date)
            .then_with(|| right_id.cmp(&left_id))
    });
    let mut selections = Vec::<LatestItemSelection>::with_capacity(limit.min(candidates.len()));
    let mut group_indexes = HashMap::<LatestItemGroupKey, usize>::new();
    for candidate in candidates {
        let item = match candidate {
            LatestGroupingCandidate::Item(item) => item,
            LatestGroupingCandidate::Tv { selection, .. } => {
                if selections.len() < limit {
                    selections.push(selection);
                }
                continue;
            }
        };

        let container = if item.item_type.eq_ignore_ascii_case("Audio") {
            audio_album_ids
                .get(&item.id)
                .and_then(|id| grouping_items.get(id))
                .filter(|container| container.item_type.eq_ignore_ascii_case("MusicAlbum"))
        } else if item.item_type.eq_ignore_ascii_case("Photo") {
            photo_album_ids
                .get(&item.id)
                .and_then(|id| grouping_items.get(id))
                .filter(|container| container.item_type.eq_ignore_ascii_case("PhotoAlbum"))
        } else {
            None
        };
        let group_key = container
            .map(|container| LatestItemGroupKey::Container(container.id))
            .or_else(|| {
                item.item_type
                    .eq_ignore_ascii_case("Movie")
                    .then(|| item.presentation_unique_key.clone())
                    .flatten()
                    .map(LatestItemGroupKey::Presentation)
            });

        let Some(group_key) = group_key else {
            if selections.len() < limit {
                selections.push(LatestItemSelection {
                    item,
                    child_count: None,
                });
            }
            continue;
        };
        if let Some(index) = group_indexes.get(&group_key).copied() {
            if let Some(container) = container {
                let selection = &mut selections[index];
                let child_count = selection.child_count.get_or_insert(1);
                *child_count = child_count.saturating_add(1);
                if selection.item.id != container.id {
                    selection.item = container.clone();
                }
            }
            continue;
        }
        if selections.len() >= limit {
            continue;
        }
        group_indexes.insert(group_key, selections.len());
        if let Some(container) = container
            && container.item_type.eq_ignore_ascii_case("MusicAlbum")
        {
            selections.push(LatestItemSelection {
                item: container.clone(),
                child_count: Some(1),
            });
        } else {
            selections.push(LatestItemSelection {
                item,
                child_count: None,
            });
        }
    }
    Ok(selections)
}

#[derive(Debug, Default)]
struct ResolvedParentScope {
    parent_id: Option<Uuid>,
    parent_ids: Vec<Uuid>,
}

async fn resolve_user_view_parent_scope(
    state: &AppState,
    authenticated_user: &jellyfin_data::entities::user::Model,
    target_user_id: Uuid,
    parent_id: Option<Uuid>,
) -> Result<ResolvedParentScope, ApiError> {
    let Some(parent_id) = parent_id.filter(|id| !id.is_nil()) else {
        return Ok(ResolvedParentScope::default());
    };
    let parent = state
        .user_library
        .item(authenticated_user, target_user_id, parent_id)
        .await?;
    if parent.item_type != "UserView" {
        return Ok(ResolvedParentScope {
            parent_id: Some(parent_id),
            parent_ids: Vec::new(),
        });
    }

    // Official UserView.GetIdsForAncestorQuery redirects synthetic views to
    // their physical display parent. Without this mapping clients can render
    // the home view but every subsequent ParentId query returns an empty page.
    let display_parent_id = parent
        .data
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|data| {
            data.get("DisplayParentId")
                .or_else(|| data.get("display_parent_id"))
        })
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .and_then(|id| Uuid::parse_str(id).ok());
    if let Some(display_parent_id) = display_parent_id {
        return Ok(ResolvedParentScope {
            parent_id: Some(display_parent_id),
            parent_ids: Vec::new(),
        });
    }

    if let Some(parent_ids) = state
        .user_views
        .grouped_content_parent_ids(target_user_id, parent_id)
        .await?
    {
        if parent_ids.is_empty() {
            return Ok(ResolvedParentScope {
                parent_id: Some(parent_id),
                parent_ids,
            });
        }
        return Ok(ResolvedParentScope {
            parent_id: None,
            parent_ids,
        });
    }

    // An unknown synthetic view must stay scoped to its own empty subtree;
    // falling back to the user root would expose unrelated libraries.
    Ok(ResolvedParentScope {
        parent_id: Some(parent_id),
        parent_ids: Vec::new(),
    })
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
            parent_ids: Vec::new(),
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
            include_alternate_versions: false,
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

fn parse_image_type_selector(value: &str) -> Option<i32> {
    let value = value.trim();
    value
        .parse::<i32>()
        .ok()
        .or_else(|| image_type_code(value).map(i32::from))
}

pub(crate) fn parse_image_type_selectors(values: &[String]) -> Vec<i32> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .filter_map(parse_image_type_selector)
        .collect()
}

const fn default_latest_limit() -> u64 {
    20
}

const fn default_true() -> bool {
    true
}

const fn default_total_record_count() -> bool {
    true
}

#[derive(Debug)]
pub(crate) struct PageDtoOptions {
    pub(crate) enable_images: bool,
    pub(crate) image_type_limit: usize,
    pub(crate) enable_image_types: Vec<i32>,
    pub(crate) enable_user_data: bool,
}

impl Default for PageDtoOptions {
    fn default() -> Self {
        Self {
            enable_images: true,
            image_type_limit: usize::MAX,
            enable_image_types: Vec::new(),
            enable_user_data: true,
        }
    }
}

pub(crate) async fn page_to_dto(
    state: &AppState,
    page: BaseItemPage,
    fields: Vec<String>,
    target_user_id: Uuid,
) -> Result<user_library::BaseItemQueryResult, ApiError> {
    page_to_dto_with_options(
        state,
        page,
        fields,
        target_user_id,
        &PageDtoOptions::default(),
    )
    .await
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn page_to_dto_with_options(
    state: &AppState,
    page: BaseItemPage,
    fields: Vec<String>,
    target_user_id: Uuid,
    dto_options: &PageDtoOptions,
) -> Result<user_library::BaseItemQueryResult, ApiError> {
    let requested_fields = user_library::BaseItemDtoFields::from_names(&fields);
    let item_ids = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
    let mut child_counts =
        user_library::child_counts_for_items(state, &page.items, requested_fields, target_user_id)
            .await?;
    let mut recursive_item_counts = user_library::recursive_item_counts_for_items(
        state,
        &page.items,
        requested_fields,
        target_user_id,
    )
    .await?;
    let mut media_source_groups = if requested_fields.wants_media_sources() {
        let sources = state
            .base_items
            .media_source_versions_for_items(&item_ids)
            .await?;
        let mut groups = std::collections::HashMap::<Uuid, Vec<_>>::new();
        for source in sources {
            groups
                .entry(source.primary_version_id.unwrap_or(source.id))
                .or_default()
                .push(source);
        }
        groups
    } else {
        std::collections::HashMap::new()
    };
    let stream_item_ids = if requested_fields.wants_media_sources() {
        media_source_groups
            .values()
            .flatten()
            .map(|item| item.id)
            .collect::<Vec<_>>()
    } else {
        item_ids.clone()
    };
    let linked_alternate_version_parents = if requested_fields.wants_media_sources() {
        state
            .base_items
            .linked_alternate_version_parents(&stream_item_ids)
            .await?
    } else {
        std::collections::HashMap::new()
    };
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
        state
            .media_streams
            .get_media_streams_for_items(&stream_item_ids)
            .await?
    } else {
        std::collections::HashMap::new()
    };
    let mut media_attachments = if requested_fields.wants_media_attachments() {
        state
            .media_attachments
            .get_media_attachments_for_items(&stream_item_ids)
            .await?
    } else {
        std::collections::HashMap::new()
    };
    let mut media_source_counts = if requested_fields.wants_media_source_count() {
        state.base_items.media_source_counts(&item_ids).await?
    } else {
        std::collections::HashMap::new()
    };
    let mut trickplay_manifests =
        user_library::trickplay_manifests_for_items(state, &page.items, requested_fields).await?;
    let mut user_dtos = if dto_options.enable_user_data {
        state
            .user_data
            .preferred_dto_map(target_user_id, &page.items)
            .await?
    } else {
        HashMap::new()
    };
    let mut relations = user_library::load_relation_metadata(state, &page.items).await?;
    let mut image_projections =
        if dto_options.enable_images || requested_fields.wants_primary_image_aspect_ratio() {
            state
                .dto_images
                .project_many(
                    &item_ids,
                    jellyfin_server_implementations::DtoImageOptions {
                        enable_images: dto_options.enable_images,
                        primary_image_limit: if dto_options.enable_images {
                            dto_options.image_type_limit
                        } else {
                            0
                        },
                        include_primary_image_aspect_ratio: requested_fields
                            .wants_primary_image_aspect_ratio(),
                    },
                )
                .await
                .map_err(|_| ApiError::Internal)?
        } else {
            HashMap::new()
        };

    let mut items = Vec::with_capacity(page.items.len());
    for item in page.items {
        let item_id = item.id;
        let media_source_group_id = item.primary_version_id.unwrap_or(item_id);
        let mut dto = user_library::item_to_dto(item, state.server_id());
        let original_language = dto.original_language.clone();
        user_library::attach_child_count(&mut dto, child_counts.remove(&item_id));
        user_library::attach_recursive_item_count(&mut dto, recursive_item_counts.remove(&item_id));
        if requested_fields.wants_media_source_count() {
            user_library::attach_media_source_count(
                &mut dto,
                media_source_counts.remove(&item_id).unwrap_or_default(),
            );
        }
        if let Some(user_data) = user_dtos.remove(&item_id) {
            user_library::attach_user_data_dto(&mut dto, user_data);
        }
        if let Some(metadata) = relations.remove(&item_id) {
            user_library::attach_relation_metadata(&mut dto, metadata);
        }
        if let Some(mut projection) = image_projections.remove(&item_id) {
            constrain_image_projection(
                &mut projection,
                &dto_options.enable_image_types,
                if dto_options.enable_images {
                    dto_options.image_type_limit
                } else {
                    0
                },
            );
            user_library::attach_dto_image_projection(&mut dto, projection);
        }
        if requested_fields.wants_media_sources()
            && let Some(source_items) = media_source_groups.remove(&media_source_group_id)
        {
            let remembered = remembered_user_data.remove(&item_id);
            user_library::project_item_dto_with_versioned_sources(
                &mut dto,
                source_items,
                state.server_id(),
                requested_fields,
                &mut media_streams,
                &mut media_attachments,
                defaults.as_ref(),
                remembered.as_ref(),
                &linked_alternate_version_parents,
            )?;
        } else if requested_fields.wants_media_streams() {
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

fn constrain_image_projection(
    projection: &mut jellyfin_server_implementations::DtoImageProjection,
    enabled_image_types: &[i32],
    image_type_limit: usize,
) {
    let includes = |image_type: &str| {
        enabled_image_types.is_empty()
            || image_type_code(image_type)
                .is_some_and(|code| enabled_image_types.contains(&i32::from(code)))
    };
    projection
        .image_tags
        .retain(|image_type, _| includes(image_type) && image_type_limit > 0);
    if !includes("Primary") || image_type_limit == 0 {
        projection.primary_image_tag = None;
        projection.series_primary_image_tag = None;
        projection.parent_primary_image_item_id = None;
        projection.parent_primary_image_tag = None;
    }
    if !includes("Backdrop") || image_type_limit == 0 {
        projection.backdrop_image_tags.clear();
        projection.parent_backdrop_image_item_id = None;
        projection.parent_backdrop_image_tags.clear();
    } else {
        projection.backdrop_image_tags.truncate(image_type_limit);
        projection
            .parent_backdrop_image_tags
            .truncate(image_type_limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_enums_bind_every_official_name_and_integer() {
        for (index, expected) in SUGGESTION_MEDIA_TYPES.iter().copied().enumerate() {
            assert_eq!(
                expected.to_ascii_lowercase().parse::<SuggestionMediaType>(),
                Ok(SuggestionMediaType(expected))
            );
            assert_eq!(
                index.to_string().parse::<SuggestionMediaType>(),
                Ok(SuggestionMediaType(expected))
            );
        }
        for (index, expected) in SUGGESTION_ITEM_TYPES.iter().copied().enumerate() {
            assert_eq!(
                expected.to_ascii_lowercase().parse::<SuggestionItemType>(),
                Ok(SuggestionItemType(expected))
            );
            assert_eq!(
                index.to_string().parse::<SuggestionItemType>(),
                Ok(SuggestionItemType(expected))
            );
        }
    }

    #[test]
    fn suggestion_enums_drop_undefined_names_and_integers() {
        assert!("Stream".parse::<SuggestionMediaType>().is_err());
        assert!("99".parse::<SuggestionMediaType>().is_err());
        assert!("PluginItem".parse::<SuggestionItemType>().is_err());
        assert!("99".parse::<SuggestionItemType>().is_err());
    }

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
