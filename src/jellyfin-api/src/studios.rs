use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use jellyfin_controller::UserError;
use jellyfin_data::{BaseItemPage, ItemValueQuery};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    item_images::{GetItemImageQuery, parse_image_type, render_item_image},
    user_library,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StudiosQuery {
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
        rename = "excludeItemTypes",
        alias = "ExcludeItemTypes",
        alias = "excludeitemtypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    exclude_item_types: Vec<String>,
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
    #[serde(default = "default_total_record_count")]
    #[serde(
        rename = "enableTotalRecordCount",
        alias = "EnableTotalRecordCount",
        alias = "enabletotalrecordcount"
    )]
    enable_total_record_count: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StudioByNameQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct StudiosResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: usize,
    start_index: i32,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<StudiosQuery>,
) -> Result<Json<StudiosResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let requested_start_index = query.start_index.unwrap_or_default();
    let enable_total_record_count = query.enable_total_record_count;
    let include_item_counts = user_library::BaseItemDtoFields::from_names(&query.fields)
        .wants_item_counts()
        || !query.include_item_types.is_empty();
    let mut item_query = ItemValueQuery {
        parent_id: query.parent_id,
        search_term: query.search_term,
        include_item_types: query.include_item_types,
        exclude_item_types: query.exclude_item_types,
        is_favorite: query.is_favorite,
        user_id: Some(target_user_id),
        name_starts_with_or_greater: query.name_starts_with_or_greater,
        name_starts_with: query.name_starts_with,
        name_less_than: query.name_less_than,
        start_index: u64::try_from(requested_start_index).unwrap_or_default(),
        limit: query
            .limit
            .filter(|limit| *limit >= 0)
            .map(|limit| u64::try_from(limit).unwrap_or_default()),
        enable_total_record_count: Some(enable_total_record_count),
        ..ItemValueQuery::default()
    };
    state
        .user_library
        .apply_item_value_policy(&authenticated.user, target_user_id, &mut item_query)
        .await?;
    if let Some(parent_id) = item_query.parent_id {
        let parent = state
            .base_items
            .get(parent_id)
            .await?
            .ok_or(ApiError::InvalidRequest)?;
        if !parent.is_folder {
            item_query.parent_id = None;
            item_query.recursive = false;
            item_query.ids = vec![parent.id];
        }
    }
    let page = state.studios.list_authorized(item_query).await?;
    let studio_ids = page
        .studios
        .iter()
        .map(|studio| studio.id)
        .collect::<Vec<_>>();
    let mut persisted_by_id = state
        .base_items
        .get_many(&studio_ids)
        .await?
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<std::collections::HashMap<_, _>>();
    let persisted_studios = page
        .studios
        .iter()
        .map(|studio| persisted_by_id.remove(&studio.id).ok_or(ApiError::Internal))
        .collect::<Result<Vec<_>, _>>()?;
    let dto_options = crate::items::PageDtoOptions {
        enable_images: query.enable_images.unwrap_or(true),
        image_type_limit: query
            .image_type_limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0)),
        enable_image_types: crate::items::parse_image_type_selectors(&query.enable_image_types),
        enable_user_data: query.enable_user_data.unwrap_or(true),
    };
    let mut projected = crate::items::page_to_dto_with_options(
        state.as_ref(),
        BaseItemPage {
            items: persisted_studios,
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        },
        query.fields,
        target_user_id,
        &dto_options,
    )
    .await?;
    if include_item_counts {
        for (dto, studio) in projected.items.iter_mut().zip(page.studios) {
            apply_studio_counts(dto, studio.item_count, studio.counts);
        }
    }
    let total_record_count = if enable_total_record_count {
        usize::try_from(page.total_record_count).unwrap_or(usize::MAX)
    } else {
        0
    };
    Ok(Json(StudiosResult {
        items: projected.items,
        total_record_count,
        start_index: requested_start_index,
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<StudioByNameQuery>,
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
    let studio = state
        .studios
        .get(&authenticated.user, target_user_id, &name, item_query)
        .await?;
    let mut dto = if target_user_exists {
        user_library::project_item_to_dto(
            &state,
            studio.item,
            target_user_id,
            user_library::BaseItemDtoFields::all(),
            None,
            None,
        )
        .await?
    } else {
        project_studio_without_user(&state, studio.item).await?
    };
    apply_studio_counts(&mut dto, studio.item_count, studio.counts);
    Ok(Json(dto))
}

async fn project_studio_without_user(
    state: &AppState,
    item: jellyfin_data::entities::base_item::Model,
) -> Result<user_library::BaseItemDto, ApiError> {
    let item_id = item.id;
    let mut dto = user_library::item_to_dto_with_fields(
        item,
        state.server_id(),
        user_library::BaseItemDtoFields::all(),
    );
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
    Ok(dto)
}

fn apply_studio_counts(
    dto: &mut user_library::BaseItemDto,
    item_count: u64,
    counts: jellyfin_data::ItemValueCounts,
) {
    dto.child_count = Some(item_count);
    dto.album_count = Some(counts.album_count);
    dto.artist_count = Some(counts.artist_count);
    dto.episode_count = Some(counts.episode_count);
    dto.movie_count = Some(counts.movie_count);
    dto.music_video_count = Some(counts.music_video_count);
    dto.program_count = Some(counts.program_count);
    dto.series_count = Some(counts.series_count);
    dto.song_count = Some(counts.song_count);
    dto.trailer_count = Some(counts.trailer_count);
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
        .studios
        .image_item(name)
        .await?
        .ok_or(jellyfin_controller::StudioError::NotFound)?;
    render_item_image(state, headers, item.id, image_type, image_index, query).await
}

const fn default_total_record_count() -> bool {
    true
}
