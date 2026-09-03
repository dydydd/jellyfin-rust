use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use jellyfin_data::PersonQuery;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    item_images::{GetItemImageQuery, parse_image_type, render_item_image},
    user_library,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PersonsQueryParams {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    #[serde(rename = "limit", alias = "Limit")]
    limit: Option<u64>,
    #[serde(rename = "searchTerm", alias = "SearchTerm")]
    search_term: Option<String>,
    #[serde(rename = "nameStartsWith", alias = "NameStartsWith")]
    name_starts_with: Option<String>,
    #[serde(rename = "nameLessThan", alias = "NameLessThan")]
    name_less_than: Option<String>,
    #[serde(rename = "nameStartsWithOrGreater", alias = "NameStartsWithOrGreater")]
    name_starts_with_or_greater: Option<String>,
    #[serde(
        default,
        rename = "filters",
        alias = "Filters",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    filters: Vec<String>,
    #[serde(default, rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
    #[serde(
        default,
        rename = "excludePersonTypes",
        alias = "ExcludePersonTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_person_types: Vec<String>,
    #[serde(
        default,
        rename = "personTypes",
        alias = "PersonTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    person_types: Vec<String>,
    #[serde(rename = "parentId", alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(rename = "appearsInItemId", alias = "AppearsInItemId")]
    appears_in_item_id: Option<Uuid>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PersonsQueryParams>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
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
                start_index: query.start_index,
                limit: query.limit,
                ..PersonQuery::default()
            },
        )
        .await?;
    let items = page
        .people
        .into_iter()
        .map(|person| user_library::person_to_dto(person, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        items,
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<PersonsQueryParams>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
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
