use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use jellyfin_data::{BaseItemPage, PersonQuery};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    item_images::{GetItemImageQuery, parse_image_type, render_item_image},
    user_library,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PersonsQueryParams {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(rename = "limit", alias = "Limit")]
    limit: Option<i32>,
    #[serde(rename = "searchTerm", alias = "SearchTerm", alias = "searchterm")]
    search_term: Option<String>,
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
        rename = "nameStartsWithOrGreater",
        alias = "NameStartsWithOrGreater",
        alias = "namestartswithorgreater"
    )]
    name_starts_with_or_greater: Option<String>,
    #[serde(
        default,
        rename = "fields",
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    filters: Vec<String>,
    #[serde(
        default,
        rename = "isFavorite",
        alias = "IsFavorite",
        alias = "isfavorite"
    )]
    is_favorite: Option<bool>,
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
        default,
        rename = "excludePersonTypes",
        alias = "ExcludePersonTypes",
        alias = "excludepersontypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_person_types: Vec<String>,
    #[serde(
        default,
        rename = "personTypes",
        alias = "PersonTypes",
        alias = "persontypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    person_types: Vec<String>,
    #[serde(rename = "parentId", alias = "ParentId", alias = "parentid")]
    parent_id: Option<Uuid>,
    #[serde(
        rename = "appearsInItemId",
        alias = "AppearsInItemId",
        alias = "appearsinitemid"
    )]
    appears_in_item_id: Option<Uuid>,
    #[serde(
        rename = "enableImages",
        alias = "EnableImages",
        alias = "enableimages"
    )]
    enable_images: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PersonByNameQueryParams {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PersonsQueryResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: u64,
    start_index: i32,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PersonsQueryParams>,
) -> Result<Json<PersonsQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let response_start_index = query.start_index.unwrap_or_default();
    let start_index = u64::try_from(response_start_index).unwrap_or_default();
    let limit = query
        .limit
        .filter(|limit| *limit > 0)
        .and_then(|limit| u64::try_from(limit).ok());
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let is_favorite = query.is_favorite.or_else(|| {
        query
            .filters
            .iter()
            .any(|filter| filter.eq_ignore_ascii_case("IsFavorite"))
            .then_some(true)
    });
    let dto_options = crate::items::PageDtoOptions {
        enable_images: query.enable_images.unwrap_or(true),
        image_type_limit: query
            .image_type_limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0)),
        enable_image_types: crate::items::parse_image_type_selectors(&query.enable_image_types),
        enable_user_data: query.enable_user_data.unwrap_or(true),
    };
    let page = state
        .persons
        .list(
            &authenticated.user,
            target_user_id,
            PersonQuery {
                parent_id: query.parent_id,
                appears_in_item_id: query.appears_in_item_id,
                search_term: query.search_term,
                person_types: query.person_types,
                exclude_person_types: query.exclude_person_types,
                is_favorite,
                user_id: Some(target_user_id),
                name_starts_with_or_greater: query.name_starts_with_or_greater,
                name_starts_with: query.name_starts_with,
                name_less_than: query.name_less_than,
                start_index,
                limit,
                ..PersonQuery::default()
            },
        )
        .await?;
    let projected = crate::items::page_to_dto_with_options(
        state.as_ref(),
        BaseItemPage {
            items: page.people.into_iter().map(|person| person.model).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        },
        query.fields,
        target_user_id,
        &dto_options,
    )
    .await?;
    Ok(Json(PersonsQueryResult {
        items: projected.items,
        total_record_count: u64::try_from(projected.total_record_count).unwrap_or(u64::MAX),
        start_index: response_start_index,
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<PersonByNameQueryParams>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let person = state
        .persons
        .get(&authenticated.user, target_user_id, &name)
        .await?;
    let mut result = crate::items::page_to_dto_all_fields(
        state.as_ref(),
        BaseItemPage {
            items: vec![person.item],
            total_record_count: 1,
            start_index: 0,
        },
        target_user_id,
    )
    .await?;
    let mut dto = result.items.pop().ok_or(ApiError::Internal)?;
    apply_person_counts(&mut dto, person.counts);
    Ok(Json(dto))
}

fn apply_person_counts(dto: &mut user_library::BaseItemDto, counts: jellyfin_data::BaseItemCounts) {
    let count = |value| u64::try_from(value).unwrap_or_default();
    dto.album_count = Some(count(counts.album_count));
    dto.artist_count = Some(count(counts.artist_count));
    dto.episode_count = Some(count(counts.episode_count));
    dto.movie_count = Some(count(counts.movie_count));
    dto.music_video_count = Some(count(counts.music_video_count));
    dto.program_count = Some(count(counts.program_count));
    dto.series_count = Some(count(counts.series_count));
    dto.song_count = Some(count(counts.song_count));
    dto.trailer_count = Some(count(counts.trailer_count));
    dto.child_count = Some(
        count(counts.album_count)
            .saturating_add(count(counts.artist_count))
            .saturating_add(count(counts.episode_count))
            .saturating_add(count(counts.movie_count))
            .saturating_add(count(counts.music_video_count))
            .saturating_add(count(counts.program_count))
            .saturating_add(count(counts.series_count))
            .saturating_add(count(counts.song_count))
            .saturating_add(count(counts.trailer_count))
            .saturating_add(count(counts.box_set_count))
            .saturating_add(count(counts.book_count)),
    );
}

pub(crate) async fn get_image(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type)): Path<(String, String)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    let image_index = query.image_index.unwrap_or(0);
    get_image_for(&state, &headers, &name, &image_type, image_index, query).await
}

pub(crate) async fn get_image_by_index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type, image_index)): Path<(String, String, i32)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    get_image_for(&state, &headers, &name, &image_type, image_index, query).await
}

async fn get_image_for(
    state: &AppState,
    headers: &HeaderMap,
    name: &str,
    image_type: &str,
    image_index: i32,
    query: GetItemImageQuery,
) -> Result<Response, ApiError> {
    let image_type = parse_image_type(image_type)?;
    let item = state
        .persons
        .image_item(name)
        .await?
        .ok_or(jellyfin_controller::PersonError::NotFound)?;
    render_item_image(state, headers, item.id, image_type, image_index, query).await
}
