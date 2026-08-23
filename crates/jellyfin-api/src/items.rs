use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
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
    let fields = query.fields.clone();
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

async fn resume_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    query: ItemsQuery,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let fields = query.fields.clone();
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
    let fields = query.fields.clone();
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
        Ok(Self {
            ids: query.ids,
            exclude_ids: Vec::new(),
            genres: query.genres,
            years: query.years,
            tags: query.tags,
            person: query.person,
            min_community_rating: query.min_community_rating,
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
            min_premiere_date: None,
            allowed_official_ratings: Vec::new(),
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

    match order_by.first().map(|(sort, _)| sort.as_str()) {
        Some(sort) if sort.eq_ignore_ascii_case("DateCreated") => {
            if descending {
                BaseItemOrder::DateCreatedDescending
            } else {
                BaseItemOrder::DateCreatedAscending
            }
        }
        Some(sort) if sort.eq_ignore_ascii_case("DatePlayed") => {
            if descending {
                BaseItemOrder::DatePlayedDescending
            } else {
                BaseItemOrder::DatePlayedAscending
            }
        }
        Some(sort) if sort.eq_ignore_ascii_case("Random") => BaseItemOrder::Random,
        Some(sort) if sort.eq_ignore_ascii_case("PremiereDate") => {
            if descending {
                BaseItemOrder::PremiereDateDescending
            } else {
                BaseItemOrder::PremiereDateAscending
            }
        }
        Some(sort) if sort.eq_ignore_ascii_case("PlayCount") => {
            if descending {
                BaseItemOrder::PlayCountDescending
            } else {
                BaseItemOrder::PlayCountAscending
            }
        }
        Some(sort) if sort.eq_ignore_ascii_case("CommunityRating") => {
            if descending {
                BaseItemOrder::CommunityRatingDescending
            } else {
                BaseItemOrder::CommunityRatingAscending
            }
        }
        Some(sort) if sort.eq_ignore_ascii_case("CriticRating") => {
            if descending {
                BaseItemOrder::CriticRatingDescending
            } else {
                BaseItemOrder::CriticRatingAscending
            }
        }
        Some(sort) if sort.eq_ignore_ascii_case("Runtime") => {
            if descending {
                BaseItemOrder::RuntimeTicksDescending
            } else {
                BaseItemOrder::RuntimeTicksAscending
            }
        }
        Some(sort)
            if sort.eq_ignore_ascii_case("SortName") || sort.eq_ignore_ascii_case("Name") =>
        {
            if descending {
                BaseItemOrder::SortNameDescending
            } else {
                BaseItemOrder::SortName
            }
        }
        _ => BaseItemOrder::default(),
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
}
