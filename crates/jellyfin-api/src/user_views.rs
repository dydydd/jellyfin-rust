use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_controller::VirtualFolder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    user_library::{BaseItemDto, BaseItemQueryResult},
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UserViewsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(
        default,
        rename = "includeExternalContent",
        alias = "IncludeExternalContent"
    )]
    include_external_content: Option<bool>,
    #[serde(
        default,
        rename = "presetViews",
        alias = "PresetViews",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    preset_views: Vec<String>,
    #[serde(default, rename = "includeHidden", alias = "IncludeHidden")]
    include_hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SpecialViewOptionDto {
    name: String,
    id: String,
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserViewsQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    user_views_for(state, headers, &uri, query.user_id, query).await
}

pub(crate) async fn get_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Query(query): Query<UserViewsQuery>,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    user_views_for(state, headers, &uri, Some(user_id), query).await
}

pub(crate) async fn grouping_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UserViewsQuery>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    grouping_options_for(state, headers, &uri, query.user_id).await
}

pub(crate) async fn grouping_options_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    grouping_options_for(state, headers, &uri, Some(user_id)).await
}

async fn user_views_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
    query: UserViewsQuery,
) -> Result<Json<BaseItemQueryResult>, ApiError> {
    let target_user_id = target_user_id(&state, &headers, uri, requested_user_id).await?;
    state.users.get(target_user_id).await?;
    let _ = query.include_external_content;
    let items = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .filter(|folder| query.include_hidden || !is_hidden(folder))
        .filter(|folder| preset_matches(folder, &query.preset_views))
        .map(|folder| view_to_dto(folder, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}

async fn grouping_options_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
) -> Result<Json<Vec<SpecialViewOptionDto>>, ApiError> {
    let target_user_id = target_user_id(&state, &headers, uri, requested_user_id).await?;
    state.users.get(target_user_id).await?;
    Ok(Json(
        state
            .virtual_folders
            .list()
            .await?
            .into_iter()
            .filter(is_eligible_for_grouping)
            .map(|folder| SpecialViewOptionDto {
                name: folder.name,
                id: folder.id.simple().to_string(),
            })
            .collect(),
    ))
}

async fn target_user_id(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.target_user_id(requested_user_id)
}

pub(crate) fn view_to_dto(folder: VirtualFolder, server_id: &str) -> BaseItemDto {
    BaseItemDto {
        name: Some(folder.name.clone()),
        server_id: server_id.to_owned(),
        id: folder.id.simple().to_string(),
        item_type: "CollectionFolder".to_owned(),
        etag: folder.id.simple().to_string(),
        date_created: None,
        sort_name: Some(folder.name),
        path: None,
        overview: None,
        media_type: None,
        collection_type: folder.collection_type,
        is_folder: true,
        is_virtual_item: false,
        parent_id: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        premiere_date: None,
        run_time_ticks: None,
        presentation_unique_key: None,
        series_id: None,
        season_id: None,
        extra_type: None,
        has_lyrics: None,
        provider_ids: None,
        media_sources: None,
        media_streams: None,
        trickplay: None,
    }
}

fn preset_matches(folder: &VirtualFolder, preset_views: &[String]) -> bool {
    preset_views.is_empty()
        || folder
            .collection_type
            .as_deref()
            .is_some_and(|collection_type| {
                preset_views
                    .iter()
                    .any(|preset| preset.eq_ignore_ascii_case(collection_type))
            })
}

fn is_eligible_for_grouping(folder: &VirtualFolder) -> bool {
    folder
        .collection_type
        .as_deref()
        .is_none_or(|collection_type| {
            collection_type.eq_ignore_ascii_case("movies")
                || collection_type.eq_ignore_ascii_case("tvshows")
        })
}

fn is_hidden(folder: &VirtualFolder) -> bool {
    bool_option(
        &folder.library_options,
        &["IsHidden", "isHidden", "Hidden", "hidden"],
    )
    .unwrap_or(false)
}

pub(crate) fn bool_option(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.as_object()?.get(*key)?.as_bool())
}
