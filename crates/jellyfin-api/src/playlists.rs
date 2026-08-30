use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::PlaylistUserPermission;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::user_library::{BaseItemDto, BaseItemDtoFields, BaseItemQueryResult, item_to_dto};
use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CreateQuery {
    #[serde(alias = "Name")]
    name: Option<String>,
    #[serde(
        alias = "Ids",
        alias = "IDs",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<Uuid>,
    #[serde(alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(alias = "MediaType")]
    media_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct CreateBody {
    name: Option<String>,
    #[serde(deserialize_with = "crate::query::comma::deserialize")]
    ids: Vec<Uuid>,
    user_id: Option<Uuid>,
    media_type: Option<String>,
    users: Vec<PlaylistUserPermission>,
    is_public: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemsQuery {
    #[serde(
        default,
        rename = "ids",
        alias = "Ids",
        alias = "IDs",
        alias = "entryIds",
        alias = "EntryIds",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<Uuid>,
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, alias = "Position")]
    position: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetItemsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: usize,
    #[serde(default, alias = "Limit")]
    limit: Option<usize>,
    #[serde(
        default,
        alias = "Fields",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    fields: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaylistCreationResult {
    id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlaylistDto {
    open_access: bool,
    shares: Vec<PlaylistUserPermission>,
    item_ids: Vec<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct UpdateBody {
    name: Option<String>,
    ids: Option<Vec<Uuid>>,
    users: Option<Vec<PlaylistUserPermission>>,
    is_public: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct UpdateUserBody {
    can_edit: Option<bool>,
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<CreateQuery>,
    body: Result<Option<Json<CreateBody>>, JsonRejection>,
) -> Result<Json<PlaylistCreationResult>, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let body = body
        .map_err(|_| ApiError::InvalidRequest)?
        .map(|Json(body)| body);
    // Upstream only applies the `CreatePlaylistDto.IsPublic` default of `true`
    // when a body was actually supplied. A bodyless request leaves
    // `PlaylistCreationRequest.Public` null, which `PlaylistManager` turns into
    // `OpenAccess = false`.
    let is_public = body
        .as_ref()
        .map_or(false, |body| body.is_public.unwrap_or(true));
    let requested_user_id = query.user_id.or_else(|| body.as_ref()?.user_id);
    let owner_user_id = identity.target_user_id(requested_user_id)?;
    if owner_user_id.is_nil() {
        return Err(ApiError::InvalidRequest);
    }
    let body = body.unwrap_or_default();
    let name = query.name.or(body.name).ok_or(ApiError::InvalidRequest)?;
    let ids = if query.ids.is_empty() {
        body.ids
    } else {
        query.ids
    };
    let media_type = query.media_type.or(body.media_type);
    let shares = body.users.as_slice();
    let id = state
        .playlists
        .create(name, owner_user_id, is_public, media_type, shares, &ids)
        .await?;
    Ok(Json(PlaylistCreationResult {
        id: id.simple().to_string(),
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(playlist_id): Path<Uuid>,
) -> Result<Json<PlaylistDto>, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    let playlist = state.playlists.get_for_user(playlist_id, user_id).await?;
    let item_ids = state.playlists.item_ids(playlist_id, user_id).await?;
    Ok(Json(PlaylistDto {
        open_access: playlist.open_access,
        shares: playlist.shares,
        item_ids,
    }))
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(playlist_id): Path<Uuid>,
    request: Result<Json<UpdateBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .playlists
        .update(
            playlist_id,
            user_id,
            request.name,
            request.ids.as_deref(),
            request.users.as_deref(),
            request.is_public,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn get_users(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(playlist_id): Path<Uuid>,
) -> Result<Json<Vec<PlaylistUserPermission>>, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    Ok(Json(state.playlists.users(playlist_id, user_id).await?))
}

pub(crate) async fn get_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((playlist_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<PlaylistUserPermission>, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    Ok(Json(
        state
            .playlists
            .user(playlist_id, user_id, target_user_id)
            .await?,
    ))
}

pub(crate) async fn set_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((playlist_id, target_user_id)): Path<(Uuid, Uuid)>,
    request: Result<Json<UpdateUserBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .playlists
        .set_user(
            playlist_id,
            user_id,
            target_user_id,
            request.can_edit.unwrap_or(false),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_user(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((playlist_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    state
        .playlists
        .remove_user(playlist_id, user_id, target_user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn add_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(playlist_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(query.user_id)?;
    if query.ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    state
        .playlists
        .add_items(playlist_id, user_id, &query.ids, query.position)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn get_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(playlist_id): Path<Uuid>,
    Query(query): Query<GetItemsQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(query.user_id)?;
    if user_id.is_nil() {
        return Err(ApiError::InvalidRequest);
    }
    let page = state
        .playlists
        .items(playlist_id, user_id, query.start_index, query.limit)
        .await?;
    let fields = BaseItemDtoFields::from_names(&query.fields);
    let defaults =
        crate::user_library::media_stream_defaults_for_user(&state, user_id, fields).await?;
    let mut items = Vec::<BaseItemDto>::with_capacity(page.items.len());
    for entry in page.items {
        let mut dto = if fields == BaseItemDtoFields::default() {
            item_to_dto(entry.item, state.server_id())
        } else {
            crate::user_library::project_item_to_dto(
                &state,
                entry.item,
                user_id,
                fields,
                defaults.as_ref(),
                None,
            )
            .await?
        };
        dto.playlist_item_id = Some(entry.entry_id.simple().to_string());
        items.push(dto);
    }
    Ok(Json(BaseItemQueryResult {
        items,
        total_record_count: page.total_record_count,
        start_index: page.start_index,
    }))
}

pub(crate) async fn move_item(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((playlist_id, item_id, new_index)): Path<(Uuid, Uuid, usize)>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    state
        .playlists
        .move_item(playlist_id, user_id, item_id, new_index)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(playlist_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    let user_id = identity.target_user_id(None)?;
    if query.ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    state
        .playlists
        .remove_items(playlist_id, user_id, &query.ids)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
