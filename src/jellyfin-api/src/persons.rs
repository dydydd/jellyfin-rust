use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use jellyfin_data::PersonQuery;
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
    let items = page
        .people
        .into_iter()
        .map(|person| user_library::person_to_dto(person, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(PersonsQueryResult {
        items,
        total_record_count: page.total_record_count,
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
    Ok(Json(user_library::person_to_dto(person, state.server_id())))
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
    let item = state
        .persons
        .image_item(name)
        .await?
        .ok_or(jellyfin_controller::PersonError::NotFound)?;
    render_item_image(
        state,
        headers,
        item.id,
        parse_image_type(image_type)?,
        image_index,
        query,
    )
    .await
}
