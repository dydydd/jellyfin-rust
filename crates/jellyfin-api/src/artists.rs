use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use jellyfin_controller::ArtistValueKind;
use jellyfin_data::ItemValueQuery;
use serde::Deserialize;
use uuid::Uuid;

use crate::item_images::{GetItemImageQuery, parse_image_type, render_item_image};
use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArtistsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    #[serde(rename = "limit", alias = "Limit")]
    limit: Option<u64>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    search_term: Option<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
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
    #[serde(default, rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
    #[serde(rename = "nameStartsWithOrGreater", alias = "NameStartsWithOrGreater")]
    name_starts_with_or_greater: Option<String>,
    #[serde(rename = "nameStartsWith", alias = "NameStartsWith")]
    name_starts_with: Option<String>,
    #[serde(rename = "nameLessThan", alias = "NameLessThan")]
    name_less_than: Option<String>,
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
    #[serde(default = "default_total_record_count")]
    #[serde(rename = "enableTotalRecordCount", alias = "EnableTotalRecordCount")]
    enable_total_record_count: bool,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    list_kind(state, headers, query, ArtistValueKind::Artist).await
}

pub(crate) async fn list_album_artists(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    list_kind(state, headers, query, ArtistValueKind::AlbumArtist).await
}

async fn list_kind(
    state: Arc<AppState>,
    headers: HeaderMap,
    query: ArtistsQuery,
    kind: ArtistValueKind,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let order = crate::query::item_value_order(&query.sort_by)?;
    let descending = descending(&query.sort_order)?;
    let enable_total_record_count = query.enable_total_record_count;
    let page = state
        .artists
        .list(
            &authenticated.user,
            target_user_id,
            kind,
            ItemValueQuery {
                parent_id: query.parent_id,
                search_term: query.search_term,
                include_item_types: query.include_item_types,
                exclude_item_types: query.exclude_item_types,
                media_types: query.media_types,
                is_favorite: query.is_favorite,
                user_id: Some(target_user_id),
                name_starts_with_or_greater: query.name_starts_with_or_greater,
                name_starts_with: query.name_starts_with,
                name_less_than: query.name_less_than,
                start_index: query.start_index,
                limit: query.limit,
                order,
                descending,
                enable_total_record_count: Some(enable_total_record_count),
                ..ItemValueQuery::default()
            },
        )
        .await?;
    let items = page
        .artists
        .iter()
        .map(|artist| user_library::artist_to_dto(artist, state.server_id()))
        .collect::<Vec<_>>();
    let total_record_count = if enable_total_record_count {
        usize::try_from(page.total_record_count).unwrap_or(usize::MAX)
    } else {
        items.len()
    };
    Ok(Json(user_library::BaseItemQueryResult {
        items,
        total_record_count,
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let artist = state
        .artists
        .get(&authenticated.user, target_user_id, &name)
        .await?;
    Ok(Json(user_library::artist_to_dto(
        &artist,
        state.server_id(),
    )))
}

pub(crate) async fn get_image(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type, image_index)): Path<(String, String, i32)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    let item = state
        .artists
        .image_item(&name)
        .await?
        .ok_or(jellyfin_controller::ArtistError::NotFound)?;
    render_item_image(
        &state,
        &headers,
        item.id,
        parse_image_type(&image_type)?,
        image_index,
        query,
    )
    .await
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
