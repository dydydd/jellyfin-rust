use std::{collections::HashMap, str::FromStr, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, Uri},
    response::Response,
    routing::get,
};
use jellyfin_controller::UserError;
use jellyfin_data::{BaseItemPage, ItemValueCounts, ItemValueQuery, entities::user};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    item_images::{GetItemImageQuery, parse_image_type, render_item_image},
    user_library,
};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/GameGenres", get(list))
        .route("/GameGenres/{name}", get(detail))
        .route("/GameGenres/{name}/Images/{image_type}", get(get_image))
        .route(
            "/GameGenres/{name}/Images/{image_type}/{image_index}",
            get(get_image_by_index),
        )
}

#[derive(Debug, Default)]
struct GameGenresQuery {
    user_id: Option<Uuid>,
    start_index: Option<i32>,
    limit: Option<i32>,
    search_term: Option<String>,
    parent_id: Option<Uuid>,
    fields: Vec<String>,
    include_item_types: Vec<String>,
    exclude_item_types: Vec<String>,
    media_types: Vec<String>,
    is_favorite: Option<bool>,
    is_liked: Option<bool>,
    is_favorite_or_liked: Option<bool>,
    is_played: Option<bool>,
    is_movie: Option<bool>,
    is_series: Option<bool>,
    is_news: Option<bool>,
    is_kids: Option<bool>,
    is_sports: Option<bool>,
    genres: Vec<String>,
    official_ratings: Vec<String>,
    tags: Vec<String>,
    years: Vec<i32>,
    studios: Vec<String>,
    studio_ids: Vec<Uuid>,
    ids: Vec<Uuid>,
    name_starts_with_or_greater: Option<String>,
    name_starts_with: Option<String>,
    name_less_than: Option<String>,
    sort_by: Vec<String>,
    sort_order: Vec<String>,
    enable_total_record_count: bool,
    enable_images: Option<bool>,
    enable_user_data: Option<bool>,
    image_type_limit: Option<i32>,
    enable_image_types: Vec<String>,
    force_empty: bool,
}

impl GameGenresQuery {
    fn parse(uri: &Uri) -> Result<Self, ApiError> {
        let mut values = HashMap::<String, Vec<String>>::new();
        for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
            values
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push(value.into_owned());
        }
        let filters = collection(&values, "filters", ',');
        let mut query = Self {
            user_id: optional_uuid(&values, "userid")?,
            start_index: optional_scalar(&values, "startindex")?,
            limit: optional_scalar(&values, "limit")?,
            search_term: optional_string(&values, "searchterm"),
            parent_id: optional_uuid(&values, "parentid")?,
            fields: collection(&values, "fields", ','),
            include_item_types: collection(&values, "includeitemtypes", ','),
            exclude_item_types: collection(&values, "excludeitemtypes", ','),
            media_types: collection(&values, "mediatypes", ','),
            is_favorite: optional_bool(&values, "isfavorite")?,
            is_liked: None,
            is_favorite_or_liked: None,
            is_played: optional_bool(&values, "isplayed")?,
            is_movie: optional_bool(&values, "ismovie")?,
            is_series: optional_bool(&values, "isseries")?,
            is_news: optional_bool(&values, "isnews")?,
            is_kids: optional_bool(&values, "iskids")?,
            is_sports: optional_bool(&values, "issports")?,
            genres: collection(&values, "genres", '|'),
            official_ratings: collection(&values, "officialratings", '|'),
            tags: collection(&values, "tags", '|'),
            years: parsed_collection(&values, "years", ','),
            studios: collection(&values, "studios", '|'),
            studio_ids: parsed_collection(&values, "studioids", '|'),
            ids: parsed_collection(&values, "ids", ','),
            name_starts_with_or_greater: optional_string(&values, "namestartswithorgreater"),
            name_starts_with: optional_string(&values, "namestartswith"),
            name_less_than: optional_string(&values, "namelessthan"),
            sort_by: collection(&values, "sortby", ','),
            sort_order: collection(&values, "sortorder", ','),
            enable_total_record_count: optional_bool(&values, "enabletotalrecordcount")?
                .unwrap_or(true),
            enable_images: optional_bool(&values, "enableimages")?,
            enable_user_data: optional_bool(&values, "enableuserdata")?,
            image_type_limit: optional_scalar(&values, "imagetypelimit")?,
            enable_image_types: collection(&values, "enableimagetypes", ','),
            force_empty: false,
        };
        for filter in filters {
            if filter.eq_ignore_ascii_case("Dislikes") || filter == "1" {
                query.is_liked = Some(false);
            } else if filter.eq_ignore_ascii_case("IsFavorite") || filter == "2" {
                query.is_favorite = Some(true);
            } else if filter.eq_ignore_ascii_case("IsFavoriteOrLikes") || filter == "3" {
                query.is_favorite_or_liked = Some(true);
            } else if filter.eq_ignore_ascii_case("IsFolder") || filter == "4" {
                // GameGenre derives from BaseItem, not Folder.
                query.force_empty = true;
            } else if filter.eq_ignore_ascii_case("IsNotFolder") || filter == "5" {
                // Every GameGenre satisfies this predicate.
            } else if filter.eq_ignore_ascii_case("IsPlayed") || filter == "6" {
                query.is_played = Some(true);
            } else if filter.eq_ignore_ascii_case("IsResumable") || filter == "7" {
                query.force_empty = true;
            } else if filter.eq_ignore_ascii_case("IsUnplayed") || filter == "8" {
                query.is_played = Some(false);
            } else if filter.eq_ignore_ascii_case("Likes") || filter == "9" {
                query.is_liked = Some(true);
            }
        }
        Ok(query)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
struct GameGenresResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: i32,
}

async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<GameGenresResult>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let query = GameGenresQuery::parse(&uri)?;
    let requested_user_id = query.user_id.filter(|user_id| !user_id.is_nil());
    let (target_user_id, policy_users) =
        resolve_target_user(&state, &identity, requested_user_id).await?;
    let requested_start_index = query.start_index.unwrap_or_default();
    let include_item_counts = !query.include_item_types.is_empty();
    let mut item_query = ItemValueQuery {
        ids: query.ids,
        parent_id: query.parent_id,
        recursive: true,
        search_term: query.search_term,
        include_item_types: emby_game_item_types(query.include_item_types),
        exclude_item_types: emby_game_item_types(query.exclude_item_types),
        media_types: query.media_types,
        is_movie: query.is_movie,
        is_series: query.is_series,
        is_news: query.is_news,
        is_kids: query.is_kids,
        is_sports: query.is_sports,
        is_favorite: query.is_favorite,
        is_liked: query.is_liked,
        is_favorite_or_liked: query.is_favorite_or_liked,
        is_played: query.is_played,
        user_id: policy_users.as_ref().map(|_| target_user_id),
        genres: query.genres,
        official_ratings: query.official_ratings,
        tags: query.tags,
        years: query.years,
        studios: query.studios,
        studio_ids: query.studio_ids,
        name_starts_with_or_greater: query.name_starts_with_or_greater,
        name_starts_with: query.name_starts_with,
        name_less_than: query.name_less_than,
        start_index: u64::try_from(requested_start_index).unwrap_or_default(),
        limit: query
            .limit
            .filter(|limit| *limit >= 0)
            .map(|limit| u64::try_from(limit).unwrap_or_default()),
        order: crate::query::item_value_order(&query.sort_by)?,
        descending: descending(&query.sort_order)?,
        enable_total_record_count: Some(query.enable_total_record_count),
        ..ItemValueQuery::default()
    };
    if query.force_empty {
        item_query
            .include_item_types
            .push("__emby_no_game_genre_source__".to_owned());
    }
    if let Some((authenticated_user, _target_user)) = &policy_users {
        state
            .user_library
            .apply_item_value_policy(authenticated_user, target_user_id, &mut item_query)
            .await?;
    } else {
        // `GetItemsByName` constructs `InternalItemsQuery(User?)` with no
        // authenticated-user fallback. Explicitly opt into the global root so
        // the default policy value cannot hide real CollectionFolder children.
        item_query.access_policy.enable_all_folders = true;
    }
    let page = state.game_genres.list_authorized(item_query).await?;
    let genre_ids = page.genres.iter().map(|genre| genre.id).collect::<Vec<_>>();
    let mut persisted_by_id = state
        .base_items
        .get_many(&genre_ids)
        .await?
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let persisted = page
        .genres
        .iter()
        .map(|genre| persisted_by_id.remove(&genre.id).ok_or(ApiError::Internal))
        .collect::<Result<Vec<_>, _>>()?;
    let dto_options = crate::items::PageDtoOptions {
        enable_images: query.enable_images.unwrap_or(true),
        image_type_limit: query
            .image_type_limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0)),
        enable_image_types: crate::items::parse_image_type_selectors(&query.enable_image_types),
        enable_user_data: policy_users.is_some() && query.enable_user_data.unwrap_or(true),
    };
    let mut projected = crate::items::page_to_dto_with_options(
        state.as_ref(),
        BaseItemPage {
            items: persisted,
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        },
        query.fields.clone(),
        target_user_id,
        &dto_options,
    )
    .await?;
    for (dto, genre) in projected.items.iter_mut().zip(page.genres) {
        normalize_game_genre(dto, false, &query.fields);
        if include_item_counts {
            apply_game_genre_counts(dto, genre.item_count, genre.counts)?;
        }
    }
    user_library::omit_incompatible_emby_relations(&uri, &mut projected.items);
    Ok(Json(GameGenresResult {
        items: projected.items,
        total_record_count: if query.enable_total_record_count {
            user_library::checked_int32(page.total_record_count)?
        } else {
            0
        },
    }))
}

async fn detail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(name): Path<String>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let user_id = optional_uuid(&query_values(&uri), "userid")?.filter(|id| !id.is_nil());
    let (target_user_id, policy_users) = resolve_target_user(&state, &identity, user_id).await?;
    let mut item_query = ItemValueQuery::default();
    if let Some((authenticated_user, _target_user)) = &policy_users {
        state
            .user_library
            .apply_item_value_policy(authenticated_user, target_user_id, &mut item_query)
            .await?;
    }
    let genre = state.game_genres.get_authorized(&name, item_query).await?;
    let mut dto = if policy_users.is_some() {
        user_library::project_item_to_dto(
            &state,
            genre.item,
            target_user_id,
            user_library::BaseItemDtoFields::all(),
            None,
            None,
        )
        .await?
    } else {
        project_without_user(&state, genre.item).await?
    };
    normalize_game_genre(&mut dto, true, &[]);
    apply_game_genre_counts(&mut dto, genre.item_count, genre.counts)?;
    user_library::omit_incompatible_emby_relations(&uri, std::slice::from_mut(&mut dto));
    Ok(Json(dto))
}

async fn project_without_user(
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

fn normalize_game_genre(dto: &mut user_library::BaseItemDto, detail: bool, fields: &[String]) {
    dto.item_type = "GameGenre".to_owned();
    dto.is_folder = false;
    dto.is_virtual_item = false;
    dto.primary_image_aspect_ratio = Some(1.0);
    if detail
        || fields
            .iter()
            .any(|field| field.eq_ignore_ascii_case("CanDelete"))
    {
        dto.can_delete = Some(false);
    }
}

fn apply_game_genre_counts(
    dto: &mut user_library::BaseItemDto,
    item_count: u64,
    counts: ItemValueCounts,
) -> Result<(), ApiError> {
    dto.child_count = Some(user_library::checked_int32(item_count)?);
    dto.album_count = Some(user_library::checked_int32(counts.album_count)?);
    dto.artist_count = Some(user_library::checked_int32(counts.artist_count)?);
    dto.episode_count = Some(user_library::checked_int32(counts.episode_count)?);
    dto.game_count = Some(user_library::checked_int32(counts.game_count)?);
    dto.movie_count = Some(user_library::checked_int32(counts.movie_count)?);
    dto.music_video_count = Some(user_library::checked_int32(counts.music_video_count)?);
    dto.program_count = Some(user_library::checked_int32(counts.program_count)?);
    dto.series_count = Some(user_library::checked_int32(counts.series_count)?);
    dto.song_count = Some(user_library::checked_int32(counts.song_count)?);
    dto.trailer_count = Some(user_library::checked_int32(counts.trailer_count)?);
    Ok(())
}

async fn resolve_target_user(
    state: &AppState,
    identity: &authentication::AuthenticatedIdentity,
    requested_user_id: Option<Uuid>,
) -> Result<(Uuid, Option<(user::Model, user::Model)>), ApiError> {
    match identity {
        authentication::AuthenticatedIdentity::Device(session) => {
            let Some(target_user_id) = requested_user_id else {
                return Ok((session.user.id, None));
            };
            if target_user_id != session.user.id && !session.user.is_administrator {
                return Err(ApiError::Forbidden);
            }
            match state.users.get(target_user_id).await {
                Ok(target_user) => Ok((target_user_id, Some((session.user.clone(), target_user)))),
                Err(UserError::NotFound) if session.user.is_administrator => {
                    Ok((target_user_id, None))
                }
                Err(error) => Err(error.into()),
            }
        }
        authentication::AuthenticatedIdentity::ApiKey(_) => {
            let Some(target_user_id) = requested_user_id else {
                return Ok((Uuid::nil(), None));
            };
            match state.users.get(target_user_id).await {
                Ok(target_user) => Ok((target_user_id, Some((target_user.clone(), target_user)))),
                Err(UserError::NotFound) => Ok((target_user_id, None)),
                Err(error) => Err(error.into()),
            }
        }
    }
}

fn emby_game_item_types(item_types: Vec<String>) -> Vec<String> {
    let mut expanded = Vec::with_capacity(item_types.len().saturating_mul(2));
    for item_type in item_types {
        let is_game = item_type.eq_ignore_ascii_case("Game")
            || item_type.eq_ignore_ascii_case("MediaBrowser.Controller.Entities.Game");
        expanded.push(item_type);
        if is_game
            && !expanded
                .iter()
                .any(|value| value == "MediaBrowser.Controller.Entities.Game")
        {
            expanded.push("MediaBrowser.Controller.Entities.Game".to_owned());
        }
        if is_game && !expanded.iter().any(|value| value == "Game") {
            expanded.push("Game".to_owned());
        }
    }
    expanded
}

#[derive(Debug, Default, Deserialize)]
struct GameGenreImageQuery {
    #[serde(flatten)]
    common: GetItemImageQuery,
    #[serde(default, rename = "Index", alias = "index")]
    index: Option<i32>,
}

async fn get_image(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<GameGenreImageQuery>,
) -> Result<Response, ApiError> {
    authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    let image_index = query.index.or(query.common.image_index).unwrap_or(0);
    get_image_for(
        &state,
        &headers,
        &name,
        &image_type,
        image_index,
        query.common,
    )
    .await
}

async fn get_image_by_index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((name, image_type, image_index)): Path<(String, String, i32)>,
    axum::extract::Query(query): axum::extract::Query<GameGenreImageQuery>,
) -> Result<Response, ApiError> {
    authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    get_image_for(
        &state,
        &headers,
        &name,
        &image_type,
        image_index,
        query.common,
    )
    .await
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
        .game_genres
        .image_item(name)
        .await?
        .ok_or(jellyfin_controller::GameGenreError::NotFound)?;
    render_item_image(state, headers, item.id, image_type, image_index, query).await
}

fn query_values(uri: &Uri) -> HashMap<String, Vec<String>> {
    let mut values = HashMap::new();
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        values
            .entry(name.to_ascii_lowercase())
            .or_insert_with(Vec::new)
            .push(value.into_owned());
    }
    values
}

fn optional_string(values: &HashMap<String, Vec<String>>, name: &str) -> Option<String> {
    values
        .get(name)
        .and_then(|values| values.last())
        .map(ToOwned::to_owned)
}

fn optional_scalar<T>(
    values: &HashMap<String, Vec<String>>,
    name: &str,
) -> Result<Option<T>, ApiError>
where
    T: FromStr,
{
    optional_string(values, name)
        .map(|value| value.parse().map_err(|_| ApiError::InvalidRequest))
        .transpose()
}

fn optional_uuid(
    values: &HashMap<String, Vec<String>>,
    name: &str,
) -> Result<Option<Uuid>, ApiError> {
    optional_scalar(values, name)
}

fn optional_bool(
    values: &HashMap<String, Vec<String>>,
    name: &str,
) -> Result<Option<bool>, ApiError> {
    let Some(value) = optional_string(values, name) else {
        return Ok(None);
    };
    if value.eq_ignore_ascii_case("true") {
        Ok(Some(true))
    } else if value.eq_ignore_ascii_case("false") {
        Ok(Some(false))
    } else {
        Err(ApiError::InvalidRequest)
    }
}

fn collection(values: &HashMap<String, Vec<String>>, name: &str, separator: char) -> Vec<String> {
    let Some(values) = values.get(name) else {
        return Vec::new();
    };
    if values.len() == 1 {
        values[0]
            .split(separator)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    } else {
        values
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }
}

fn parsed_collection<T>(
    values: &HashMap<String, Vec<String>>,
    name: &str,
    separator: char,
) -> Vec<T>
where
    T: FromStr,
{
    collection(values, name, separator)
        .into_iter()
        .filter_map(|value| value.parse().ok())
        .collect()
}

fn descending(sort_order: &[String]) -> Result<bool, ApiError> {
    let Some(order) = sort_order.first() else {
        return Ok(false);
    };
    Ok(matches!(
        crate::query::parse_sort_order(order)?,
        jellyfin_model::SortOrder::Descending
    ))
}

#[cfg(test)]
mod tests {
    use super::GameGenresQuery;

    #[test]
    fn query_binding_is_case_insensitive_and_preserves_sdk_collection_shape() {
        let uri = "/GameGenres?sTaRtInDeX=-1&LiMiT=-2&FiElDs=Genres,Studios&FIELDS=Path&IsFavorite=TRUE&Years=2024,bad&FilterS=IsNotFolder,Likes"
            .parse()
            .expect("URI");
        let query = GameGenresQuery::parse(&uri).expect("query");
        assert_eq!(query.start_index, Some(-1));
        assert_eq!(query.limit, Some(-2));
        assert_eq!(query.fields, ["Genres,Studios", "Path"]);
        assert_eq!(query.is_favorite, Some(true));
        assert_eq!(query.is_liked, Some(true));
        assert_eq!(query.years, [2024]);
        assert!(!query.force_empty);
    }
}
