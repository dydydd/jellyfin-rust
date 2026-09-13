use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use jellyfin_controller::{UserError, YearItem};
use jellyfin_data::{BaseItemPage, BaseItemQuery, ProductionYearOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct YearsQuery {
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
    #[serde(rename = "parentId", alias = "ParentId", alias = "parentid")]
    parent_id: Option<Uuid>,
    #[serde(
        default = "default_recursive",
        rename = "recursive",
        alias = "Recursive"
    )]
    recursive: bool,
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
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct YearByNameQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct YearsResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: i32,
    start_index: i32,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<YearsQuery>,
) -> Result<Json<YearsResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let order = production_year_order(&query.sort_by, &query.sort_order)?;
    let requested_start_index = query.start_index.unwrap_or_default();
    let mut item_query = BaseItemQuery {
        parent_id: query.parent_id,
        recursive: query.recursive,
        include_item_types: query.include_item_types,
        exclude_item_types: query.exclude_item_types,
        media_types: query.media_types,
        // LINQ Skip treats negative counts as zero, while the response echoes
        // the original signed StartIndex.
        start_index: u64::try_from(requested_start_index).unwrap_or_default(),
        // LINQ Take returns no items for a non-positive count.
        limit: query
            .limit
            .map(|limit| u64::try_from(limit).unwrap_or_default()),
        ..BaseItemQuery::default()
    };
    if authenticated.user.id != target_user_id && !authenticated.user.is_administrator {
        return Err(ApiError::Forbidden);
    }
    state
        .user_library
        .apply_user_policy(&mut item_query, target_user_id)
        .await?;
    let mut recursive_folder_id = None;
    if let Some(parent_id) = item_query.parent_id {
        let parent = state
            .base_items
            .get(parent_id)
            .await?
            .ok_or(ApiError::InvalidRequest)?;
        if parent.is_folder {
            recursive_folder_id = item_query.recursive.then_some(parent.id);
        } else {
            item_query.parent_id = None;
            item_query.recursive = false;
            item_query.ids = vec![parent.id];
        }
    } else if item_query.recursive {
        recursive_folder_id = Some(state.base_items.ensure_user_root().await?.id);
    }
    let recursive_item_total = if let Some(parent_id) = recursive_folder_id {
        let mut count_query = item_query.clone();
        count_query.parent_id = Some(parent_id);
        count_query.start_index = 0;
        count_query.limit = None;
        Some(
            u64::try_from(state.base_items.item_counts(&count_query).await?.item_count)
                .unwrap_or_default(),
        )
    } else {
        None
    };
    let page = state.years.list_authorized(item_query, order).await?;
    let year_ids = page.years.iter().map(|year| year.id).collect::<Vec<_>>();
    let mut persisted_by_id = state
        .base_items
        .get_many(&year_ids)
        .await?
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<std::collections::HashMap<_, _>>();
    let persisted_years = page
        .years
        .iter()
        .map(|year| persisted_by_id.remove(&year.id).ok_or(ApiError::Internal))
        .collect::<Result<Vec<_>, _>>()?;
    let dto_options = crate::items::PageDtoOptions {
        enable_images: query.enable_images.unwrap_or(true),
        image_type_limit: query
            .image_type_limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0)),
        enable_image_types: crate::items::parse_image_type_selectors(&query.enable_image_types),
        enable_user_data: query.enable_user_data.unwrap_or(true),
    };
    // Official `GetYears` projects the persisted item-by-name rows through
    // `GetItemByNameDto`, carrying the endpoint's DTO options with it.
    let projected = crate::items::page_to_dto_with_options(
        state.as_ref(),
        BaseItemPage {
            items: persisted_years,
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        },
        query.fields,
        target_user_id,
        &dto_options,
    )
    .await?;
    Ok(Json(YearsResult {
        items: projected.items,
        total_record_count: user_library::checked_int32(
            recursive_item_total.unwrap_or(page.total_record_count),
        )?,
        start_index: requested_start_index,
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(year): Path<i32>,
    Query(query): Query<YearByNameQuery>,
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
    let mut count_query = year_item_count_query(year);
    if target_user_exists {
        state
            .user_library
            .apply_user_policy(&mut count_query, target_user_id)
            .await?;
    }
    let year = state
        .years
        .get(&authenticated.user, target_user_id, year)
        .await?;
    let YearItem::Persisted(item) = year else {
        return Err(ApiError::Internal);
    };
    let mut dto = if target_user_exists {
        user_library::project_item_to_dto(
            &state,
            item,
            target_user_id,
            user_library::BaseItemDtoFields::all(),
            None,
            None,
        )
        .await?
    } else {
        project_year_without_user(&state, item).await?
    };
    apply_year_counts(&mut dto, state.base_items.item_counts(&count_query).await?)?;
    Ok(Json(dto))
}

fn year_item_count_query(year: i32) -> BaseItemQuery {
    BaseItemQuery {
        years: vec![year],
        include_item_types: [
            "Audio",
            "Episode",
            "Movie",
            "MusicAlbum",
            "MusicArtist",
            "MusicVideo",
            "Series",
            "Trailer",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        ..BaseItemQuery::default()
    }
}

fn apply_year_counts(
    dto: &mut user_library::BaseItemDto,
    counts: jellyfin_data::BaseItemCounts,
) -> Result<(), ApiError> {
    let album_count = user_library::checked_int32(counts.album_count)?;
    let artist_count = user_library::checked_int32(counts.artist_count)?;
    let episode_count = user_library::checked_int32(counts.episode_count)?;
    let movie_count = user_library::checked_int32(counts.movie_count)?;
    let music_video_count = user_library::checked_int32(counts.music_video_count)?;
    let program_count = user_library::checked_int32(counts.program_count)?;
    let series_count = user_library::checked_int32(counts.series_count)?;
    let song_count = user_library::checked_int32(counts.song_count)?;
    let trailer_count = user_library::checked_int32(counts.trailer_count)?;
    let child_count = [
        album_count,
        artist_count,
        episode_count,
        movie_count,
        music_video_count,
        program_count,
        series_count,
        song_count,
        trailer_count,
    ]
    .into_iter()
    .try_fold(0_i32, |total, count| {
        total.checked_add(count).ok_or(ApiError::Internal)
    })?;
    dto.album_count = Some(album_count);
    dto.artist_count = Some(artist_count);
    dto.episode_count = Some(episode_count);
    dto.movie_count = Some(movie_count);
    dto.music_video_count = Some(music_video_count);
    dto.program_count = Some(program_count);
    dto.series_count = Some(series_count);
    dto.song_count = Some(song_count);
    dto.trailer_count = Some(trailer_count);
    dto.child_count = Some(child_count);
    Ok(())
}

async fn project_year_without_user(
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

fn production_year_order(
    sort_by: &[String],
    sort_order: &[String],
) -> Result<ProductionYearOrder, ApiError> {
    let Some(order) = sort_by.first() else {
        return production_year_direction(sort_order);
    };

    if order.eq_ignore_ascii_case("Default")
        || order.eq_ignore_ascii_case("ProductionYear")
        || order.eq_ignore_ascii_case("SortName")
        || order.eq_ignore_ascii_case("Name")
    {
        production_year_direction(sort_order)
    } else if order.eq_ignore_ascii_case("Random") {
        Ok(ProductionYearOrder::Random)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

fn production_year_direction(sort_order: &[String]) -> Result<ProductionYearOrder, ApiError> {
    if descending(sort_order)? {
        Ok(ProductionYearOrder::Descending)
    } else {
        Ok(ProductionYearOrder::Ascending)
    }
}

const fn default_recursive() -> bool {
    true
}
