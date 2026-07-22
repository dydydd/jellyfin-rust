use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use jellyfin_controller::{MusicGenre, Person, RelatedItemKind};
use jellyfin_data::entities::base_item;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserIdQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    pub name: Option<String>,
    pub server_id: String,
    pub id: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub etag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub is_folder: bool,
    pub is_virtual_item: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_unique_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_lyrics: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemQueryResult {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: usize,
    pub start_index: usize,
}

pub(crate) async fn get_root_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_root_for(state, headers, Some(user_id)).await
}

pub(crate) async fn get_root(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_root_for(state, headers, query.user_id).await
}

pub(crate) async fn get_item_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_item_for(state, headers, Some(user_id), item_id).await
}

pub(crate) async fn get_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemDto>, ApiError> {
    get_item_for(state, headers, query.user_id, item_id).await
}

pub(crate) async fn get_intros_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    get_related_query_for(
        state,
        headers,
        Some(user_id),
        item_id,
        RelatedItemKind::Intro,
    )
    .await
}

pub(crate) async fn get_intros(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    get_related_query_for(
        state,
        headers,
        query.user_id,
        item_id,
        RelatedItemKind::Intro,
    )
    .await
}

pub(crate) async fn get_local_trailers_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        Some(user_id),
        item_id,
        RelatedItemKind::LocalTrailer,
    )
    .await
}

pub(crate) async fn get_local_trailers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        query.user_id,
        item_id,
        RelatedItemKind::LocalTrailer,
    )
    .await
}

pub(crate) async fn get_special_features_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        Some(user_id),
        item_id,
        RelatedItemKind::SpecialFeature,
    )
    .await
}

pub(crate) async fn get_special_features(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UserIdQuery>,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    get_related_for(
        state,
        headers,
        query.user_id,
        item_id,
        RelatedItemKind::SpecialFeature,
    )
    .await
}

pub(crate) async fn get_lyrics_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    get_lyrics_for(state, headers, Some(user_id), item_id).await
}

pub(crate) async fn get_lyrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    get_lyrics_for(state, headers, None, item_id).await
}

async fn get_root_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
) -> Result<Json<BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let item = state
        .user_library
        .root(&authenticated.user, target_user_id)
        .await?;
    Ok(Json(item_to_dto(item, state.server_id())))
}

async fn get_item_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Json<BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let item = state
        .user_library
        .item(&authenticated.user, target_user_id, item_id)
        .await?;
    Ok(Json(item_to_dto(item, state.server_id())))
}

async fn get_related_query_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    kind: RelatedItemKind,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let items = related_items(state.clone(), headers, requested_user_id, item_id, kind).await?;
    let items = items
        .into_iter()
        .map(|item| item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}

async fn get_related_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    kind: RelatedItemKind,
) -> Result<Json<Vec<BaseItemDto>>, ApiError> {
    let items = related_items(state.clone(), headers, requested_user_id, item_id, kind).await?;
    Ok(Json(
        items
            .into_iter()
            .map(|item| item_to_dto(item, state.server_id()))
            .collect(),
    ))
}

async fn related_items(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
    kind: RelatedItemKind,
) -> Result<Vec<base_item::Model>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    Ok(state
        .user_library
        .related_items(&authenticated.user, target_user_id, item_id, kind)
        .await?)
}

async fn get_lyrics_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    requested_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Json<Value>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = requested_user_id.unwrap_or(authenticated.user.id);
    let lyrics = state
        .user_library
        .lyrics(&authenticated.user, target_user_id, item_id)
        .await?;
    Ok(Json(lyrics))
}

pub(crate) fn item_to_dto(item: base_item::Model, server_id: &str) -> BaseItemDto {
    let extra_type = metadata_string(item.data.as_ref(), &["ExtraType", "extra_type"]);
    let has_lyrics = item
        .data
        .as_ref()
        .and_then(Value::as_object)
        .map(|object| object.contains_key("Lyrics") || object.contains_key("lyrics"));
    BaseItemDto {
        name: item.name,
        server_id: server_id.to_owned(),
        id: item.id.simple().to_string(),
        item_type: item.item_type,
        etag: item.row_version.to_string(),
        date_created: Some(item.date_created.to_rfc3339()),
        sort_name: item.sort_name,
        path: item.path,
        overview: item.overview,
        media_type: item.media_type,
        is_folder: item.is_folder,
        is_virtual_item: item.is_virtual_item,
        parent_id: item.parent_id.map(|id| id.simple().to_string()),
        index_number: item.index_number,
        parent_index_number: item.parent_index_number,
        production_year: item.production_year,
        run_time_ticks: item.runtime_ticks,
        presentation_unique_key: item.presentation_unique_key,
        series_id: item.series_id.map(|id| id.simple().to_string()),
        season_id: item.season_id.map(|id| id.simple().to_string()),
        extra_type,
        has_lyrics,
        provider_ids: metadata_value(item.data.as_ref(), &["ProviderIds", "provider_ids"]),
    }
}

pub(crate) fn music_genre_to_dto(genre: &MusicGenre, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(genre.name.clone()),
        server_id: server_id.to_owned(),
        id: genre.id.simple().to_string(),
        item_type: "MusicGenre".to_owned(),
        etag: genre.id.simple().to_string(),
        date_created: None,
        sort_name: Some(genre.name.clone()),
        path: None,
        overview: None,
        media_type: None,
        is_folder: true,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("MusicGenre-{}", genre.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
    }
}

pub(crate) fn person_to_dto(person: &Person, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(person.model.name.clone()),
        server_id: server_id.to_owned(),
        id: person.model.id.simple().to_string(),
        item_type: "Person".to_owned(),
        etag: person.model.row_version.to_string(),
        date_created: Some(person.model.date_created.to_rfc3339()),
        sort_name: Some(person.model.name.clone()),
        path: None,
        overview: None,
        media_type: None,
        is_folder: false,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        run_time_ticks: None,
        presentation_unique_key: Some(format!("Person-{}", person.model.name)),
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: Some(person.model.provider_ids.clone()),
    }
}

fn metadata_string(data: Option<&Value>, keys: &[&str]) -> Option<String> {
    let object = data?.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn metadata_value(data: Option<&Value>, keys: &[&str]) -> Option<Value> {
    let object = data?.as_object()?;
    keys.iter().find_map(|key| object.get(*key)).cloned()
}
