use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use axum_extra::extract::Query;
use jellyfin_controller::VirtualFolder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiError, AppState, authorization};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CreateQuery {
    #[serde(default, alias = "Name")]
    name: Option<String>,
    #[serde(
        rename = "collectionType",
        alias = "CollectionType",
        alias = "collectiontype"
    )]
    collection_type: Option<String>,
    #[serde(
        default,
        alias = "Paths",
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    paths: Vec<String>,
    #[serde(
        default,
        rename = "refreshLibrary",
        alias = "RefreshLibrary",
        alias = "refreshlibrary"
    )]
    refresh_library: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct AddVirtualFolderDto {
    #[serde(
        rename = "LibraryOptions",
        alias = "libraryOptions",
        alias = "libraryoptions"
    )]
    library_options: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateLibraryOptionsDto {
    #[serde(rename = "Id", alias = "id")]
    id: Uuid,
    #[serde(
        rename = "LibraryOptions",
        alias = "libraryOptions",
        alias = "libraryoptions"
    )]
    library_options: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MediaPathDto {
    #[serde(rename = "Name", alias = "name")]
    name: String,
    #[serde(rename = "Path", alias = "path")]
    path: Option<String>,
    #[serde(rename = "PathInfo", alias = "pathInfo", alias = "pathinfo")]
    path_info: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateMediaPathDto {
    #[serde(rename = "Name", alias = "name")]
    name: String,
    #[serde(rename = "PathInfo", alias = "pathInfo", alias = "pathinfo")]
    path_info: Value,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RenameQuery {
    #[serde(default, alias = "Name")]
    name: Option<String>,
    #[serde(rename = "newName", alias = "NewName", alias = "newname")]
    new_name: Option<String>,
    #[serde(
        default,
        rename = "refreshLibrary",
        alias = "RefreshLibrary",
        alias = "refreshlibrary"
    )]
    refresh_library: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DeleteQuery {
    #[serde(default, alias = "Name")]
    name: Option<String>,
    #[serde(
        default,
        rename = "refreshLibrary",
        alias = "RefreshLibrary",
        alias = "refreshlibrary"
    )]
    refresh_library: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RemovePathQuery {
    #[serde(default, alias = "Name")]
    name: Option<String>,
    #[serde(default, alias = "Path")]
    path: Option<String>,
    #[serde(
        default,
        rename = "refreshLibrary",
        alias = "RefreshLibrary",
        alias = "refreshlibrary"
    )]
    refresh_library: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct VirtualFolderQuery {
    #[serde(rename = "StartIndex", alias = "startIndex", alias = "startindex")]
    start_index: i32,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct RemoveVirtualFolder {
    #[serde(alias = "id", alias = "ID")]
    id: String,
    #[serde(default, alias = "refreshLibrary", alias = "refreshlibrary")]
    refresh_library: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct RemoveMediaPath {
    #[serde(alias = "id", alias = "ID")]
    id: String,
    #[serde(alias = "path")]
    path: String,
    #[serde(default, alias = "refreshLibrary", alias = "refreshlibrary")]
    refresh_library: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VirtualFolderQueryResult {
    items: Vec<VirtualFolderInfo>,
    total_record_count: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VirtualFolderInfo {
    name: String,
    locations: Vec<String>,
    collection_type: Option<String>,
    library_options: Value,
    item_id: String,
    primary_image_item_id: Option<String>,
    refresh_progress: Option<f64>,
    refresh_status: Option<String>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<VirtualFolderInfo>>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let mut folders = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .map(folder_info)
        .collect::<Vec<_>>();
    adapt_emby_virtual_folders(&uri, &mut folders);
    Ok(Json(folders))
}

pub(crate) async fn query(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<VirtualFolderQuery>,
) -> Result<Json<VirtualFolderQueryResult>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let mut folders = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .map(folder_info)
        .collect::<Vec<_>>();
    adapt_emby_virtual_folders(&uri, &mut folders);
    Ok(Json(page_folders(folders, query)))
}

fn adapt_emby_virtual_folders(uri: &axum::http::Uri, folders: &mut [VirtualFolderInfo]) {
    if !is_emby_protocol_uri(uri) {
        return;
    }
    for folder in folders {
        filter_emby_library_image_options(&mut folder.library_options);
    }
}

fn is_emby_protocol_uri(uri: &axum::http::Uri) -> bool {
    uri.path()
        .split('/')
        .find(|segment| !segment.is_empty())
        .is_some_and(|segment| segment.eq_ignore_ascii_case("emby"))
}

fn filter_emby_library_image_options(options: &mut Value) {
    let Some(options) = options.as_object_mut() else {
        return;
    };
    let Some(Value::Array(type_options)) = object_value_mut_ignore_case(options, "TypeOptions")
    else {
        return;
    };
    for type_option in type_options {
        let Some(type_option) = type_option.as_object_mut() else {
            continue;
        };
        let Some(Value::Array(image_options)) =
            object_value_mut_ignore_case(type_option, "ImageOptions")
        else {
            continue;
        };
        image_options.retain(|option| {
            option
                .as_object()
                .and_then(|option| object_value_ignore_case(option, "Type"))
                .and_then(Value::as_str)
                .is_none_or(|image_type| !image_type.eq_ignore_ascii_case("Profile"))
        });
    }
}

fn object_value_mut_ignore_case<'a>(
    object: &'a mut serde_json::Map<String, Value>,
    expected: &str,
) -> Option<&'a mut Value> {
    let key = object
        .keys()
        .find(|key| key.eq_ignore_ascii_case(expected))?
        .clone();
    object.get_mut(&key)
}

fn object_value_ignore_case<'a>(
    object: &'a serde_json::Map<String, Value>,
    expected: &str,
) -> Option<&'a Value> {
    object
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case(expected).then_some(value))
}

fn page_folders(
    folders: Vec<VirtualFolderInfo>,
    query: VirtualFolderQuery,
) -> VirtualFolderQueryResult {
    let total_record_count = folders.len().min(i32::MAX as usize) as i32;
    let start = query.start_index.max(0) as usize;
    let end = match query.limit {
        Some(limit) if limit <= 0 => start,
        Some(limit) => start.saturating_add(limit as usize),
        None => folders.len(),
    }
    .min(folders.len());
    let items = if start < folders.len() {
        folders[start..end].to_vec()
    } else {
        Vec::new()
    };
    VirtualFolderQueryResult {
        items,
        total_record_count,
    }
}

pub(crate) async fn delete_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<RemoveVirtualFolder>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let body = request.map_err(|_| ApiError::InvalidRequest)?.0;
    let id = Uuid::parse_str(&body.id).map_err(|_| ApiError::InvalidRequest)?;
    let folder = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .find(|folder| folder.id == id)
        .ok_or(ApiError::NotFound)?;
    state
        .virtual_folders
        .delete(&folder.name, body.refresh_library)
        .await?;
    Ok(axum::http::StatusCode::OK.into_response())
}

pub(crate) async fn remove_path_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<RemoveMediaPath>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let body = request.map_err(|_| ApiError::InvalidRequest)?.0;
    let id = Uuid::parse_str(&body.id).map_err(|_| ApiError::InvalidRequest)?;
    let folder = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .find(|folder| folder.id == id)
        .ok_or(ApiError::NotFound)?;
    state
        .virtual_folders
        .remove_path(&folder.name, &body.path, body.refresh_library)
        .await?;
    Ok(axum::http::StatusCode::OK.into_response())
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<CreateQuery>,
    request: Result<Option<Json<AddVirtualFolderDto>>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let name = required_query(query.name)?;
    let body = request
        .map_err(|_| ApiError::InvalidRequest)?
        .map(|body| body.0);
    if is_emby_protocol_uri(&uri) && body.is_none() {
        return Err(ApiError::InvalidRequest);
    }
    let options = body
        .and_then(|body| body.library_options)
        .unwrap_or_else(|| json!({ "Enabled": true, "PathInfos": [] }));
    let refresh_after_create = query.refresh_library;
    state
        .virtual_folders
        .create(
            &name,
            query.collection_type,
            options,
            query.paths,
            query.refresh_library,
        )
        .await?;
    let summary = refresh_library_if_requested(&state, refresh_after_create).await?;
    broadcast_scan_summary(&state, summary.as_ref()).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<DeleteQuery>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let name = required_query(query.name)?;
    state
        .virtual_folders
        .delete(&name, query.refresh_library)
        .await?;
    crate::websocket::broadcast_library_changed(&state, &[], &[], &[]).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn rename(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<RenameQuery>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let name = required_query(query.name)?;
    let new_name = required_query(query.new_name)?;
    state
        .virtual_folders
        .rename(&name, &new_name, query.refresh_library)
        .await?;
    crate::websocket::broadcast_library_changed(&state, &[], &[], &[]).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn add_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<DeleteQuery>,
    request: Result<Json<MediaPathDto>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let body = request.map_err(|_| ApiError::InvalidRequest)?.0;
    if body.name.trim().is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    let path_info = body
        .path_info
        .or_else(|| body.path.map(|path| json!({ "Path": path })))
        .ok_or(ApiError::InvalidRequest)?;
    state
        .virtual_folders
        .add_path(&body.name, path_info, query.refresh_library)
        .await?;
    let summary = refresh_library_if_requested(&state, query.refresh_library).await?;
    broadcast_scan_summary(&state, summary.as_ref()).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn update_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<DeleteQuery>,
    request: Result<Json<UpdateMediaPathDto>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let body = request.map_err(|_| ApiError::InvalidRequest)?.0;
    if body.name.trim().is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    state
        .virtual_folders
        .update_path(&body.name, body.path_info)
        .await?;
    let summary = refresh_library_if_requested(&state, query.refresh_library).await?;
    broadcast_scan_summary(&state, summary.as_ref()).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn remove_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<RemovePathQuery>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let name = required_query(query.name)?;
    let path = required_query(query.path)?;
    state
        .virtual_folders
        .remove_path(&name, &path, query.refresh_library)
        .await?;
    crate::websocket::broadcast_library_changed(&state, &[], &[], &[]).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn update_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<UpdateLibraryOptionsDto>, JsonRejection>,
) -> Result<Response, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let body = request.map_err(|_| ApiError::InvalidRequest)?.0;
    state
        .virtual_folders
        .update_options(
            body.id,
            body.library_options.ok_or(ApiError::InvalidRequest)?,
        )
        .await?;
    crate::websocket::broadcast_library_changed(&state, &[], &[], &[]).await;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

fn required_query(value: Option<String>) -> Result<String, ApiError> {
    let value = value.ok_or(ApiError::InvalidRequest)?;
    if value.trim().is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    Ok(value)
}

async fn refresh_library_if_requested(
    state: &AppState,
    refresh_library: bool,
) -> Result<Option<jellyfin_controller::LibraryScanSummary>, ApiError> {
    if refresh_library {
        Ok(Some(state.library_scan.scan_all().await?))
    } else {
        Ok(None)
    }
}

async fn broadcast_scan_summary(
    state: &AppState,
    summary: Option<&jellyfin_controller::LibraryScanSummary>,
) {
    let (added, removed, updated): (&[Uuid], &[Uuid], &[Uuid]) = match summary {
        Some(summary) => (
            summary.added_ids.as_slice(),
            summary.removed_ids.as_slice(),
            summary.changed_ids.as_slice(),
        ),
        None => (&[], &[], &[]),
    };
    crate::websocket::broadcast_library_changed(state, added, removed, updated).await;
}

fn folder_info(folder: VirtualFolder) -> VirtualFolderInfo {
    VirtualFolderInfo {
        name: folder.name,
        locations: folder.locations,
        collection_type: folder.collection_type,
        library_options: folder.library_options,
        item_id: folder.id.simple().to_string(),
        primary_image_item_id: None,
        refresh_progress: None,
        refresh_status: folder
            .refresh_requested
            .then(|| "RefreshRequested".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emby_query_paging_preserves_total_and_empty_limit() {
        let folders = (0..2)
            .map(|index| VirtualFolderInfo {
                name: format!("Library {index}"),
                locations: Vec::new(),
                collection_type: None,
                library_options: Value::Null,
                item_id: index.to_string(),
                primary_image_item_id: None,
                refresh_progress: None,
                refresh_status: None,
            })
            .collect();
        let page = page_folders(
            folders,
            VirtualFolderQuery {
                start_index: -1,
                limit: Some(0),
            },
        );
        assert_eq!(page.total_record_count, 2);
        assert!(page.items.is_empty());
    }
}
