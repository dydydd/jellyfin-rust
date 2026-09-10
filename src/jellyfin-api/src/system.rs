use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, OriginalUri, Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, Response, StatusCode, header},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use jellyfin_controller::SystemLogFile;
use jellyfin_model::{
    EndPointInfo, LibraryStorageDto, PublicSystemInfo, SystemInfo, SystemStorageDto,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::ReaderStream;

use crate::{ApiError, AppState, SystemCommand, authentication, authorization, startup};

const STREAM_BUFFER_SIZE: usize = 64 * 1024;
const TEXT_UTF8: HeaderValue = HeaderValue::from_static("text/plain; charset=utf-8");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct LogFileQuery {
    #[serde(alias = "Name")]
    name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct LogFileDto {
    date_created: DateTime<Utc>,
    date_modified: DateTime<Utc>,
    size: i64,
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LogQuery {
    #[serde(alias = "StartIndex", alias = "startindex")]
    start_index: Option<i32>,
    #[serde(alias = "Limit")]
    limit: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct LogQueryResult {
    items: Vec<LogFileDto>,
    total_record_count: usize,
}

pub(crate) async fn query_logs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<LogQuery>, QueryRejection>,
) -> Result<Json<LogQueryResult>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let all = state.system_logs.list().await;
    let total_record_count = all.len();
    let start = query.start_index.unwrap_or(0).max(0) as usize;
    let count = query.limit.map_or(usize::MAX, |n| n.max(0) as usize);
    let items = all
        .into_iter()
        .skip(start)
        .take(count)
        .map(LogFileDto::from)
        .collect();
    Ok(Json(LogQueryResult {
        items,
        total_record_count,
    }))
}

pub(crate) async fn get_logs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<LogFileDto>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(
        state
            .system_logs
            .list()
            .await
            .into_iter()
            .map(LogFileDto::from)
            .collect(),
    ))
}

pub(crate) async fn get_log_file(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<LogFileQuery>, QueryRejection>,
) -> Result<Response<Body>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let name = query
        .name
        .filter(|name| !name.trim().is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    stream_log_file(&state, name).await
}

/// Emby's generated clients use `/System/Logs/{Name}` for the same stream
/// exposed by Jellyfin's `/System/Logs/Log?Name=...` endpoint.
pub(crate) async fn get_log_file_by_name(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response<Body>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    stream_log_file(&state, name).await
}

/// Emby's legacy route returns log contents as a JSON line query result.
pub async fn emby_log_file_lines(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(name): Path<String>,
    query: Result<Query<LogLinesQuery>, QueryRejection>,
) -> Result<Response<Body>, Response<Body>> {
    require_elevated(&state, &headers, &uri)
        .await
        .map_err(IntoResponse::into_response)?;
    let Query(query) = query
        .map_err(|_| ApiError::InvalidRequest)
        .map_err(IntoResponse::into_response)?;
    let file = state
        .system_logs
        .open(&name)
        .await
        .map_err(ApiError::from)
        .map_err(IntoResponse::into_response)?
        .into_file();
    let (items, total_record_count) =
        read_log_lines(file, query.start_index.unwrap_or_default(), query.limit)
            .await
            .map_err(|_| ApiError::Internal)
            .map_err(IntoResponse::into_response)?;
    Ok(Json(LogLinesResult {
        items,
        total_record_count,
    })
    .into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LogLinesQuery {
    #[serde(alias = "StartIndex", alias = "startindex")]
    start_index: Option<i32>,
    #[serde(alias = "Limit")]
    limit: Option<i32>,
}

async fn read_log_lines(
    file: tokio::fs::File,
    start_index: i32,
    limit: Option<i32>,
) -> std::io::Result<(Vec<String>, i32)> {
    let start_index = start_index.max(0) as usize;
    let limit = limit.and_then(|limit| (limit >= 0).then_some(limit as usize));
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut line_index = 0usize;
    let mut total_record_count = 0usize;
    let mut items = Vec::new();
    while reader.read_until(b'\n', &mut line).await? != 0 {
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line_index >= start_index && limit.is_none_or(|limit| items.len() < limit) {
            items.push(String::from_utf8_lossy(&line).into_owned());
        }
        line_index += 1;
        total_record_count = total_record_count.saturating_add(1);
        line.clear();
    }
    Ok((items, i32::try_from(total_record_count).unwrap_or(i32::MAX)))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LogLinesResult {
    items: Vec<String>,
    total_record_count: i32,
}

async fn stream_log_file(state: &AppState, name: String) -> Result<Response<Body>, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    let log = state.system_logs.open(&name).await?;
    let stream = ReaderStream::with_capacity(log.into_file(), STREAM_BUFFER_SIZE);

    Response::builder()
        .header(header::CONTENT_TYPE, TEXT_UTF8)
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError::Internal)
}

pub(crate) async fn storage(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<SystemStorageDto>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let libraries = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .map(|library| LibraryStorageDto {
            id: library.id,
            name: library.name,
            folders: library
                .locations
                .into_iter()
                .map(|path| state.system_storage.folder(path))
                .collect(),
        })
        .collect();
    Ok(Json(SystemStorageDto {
        program_data_folder: state.system_storage.folder(&state.program_data_directory),
        web_folder: state.system_storage.folder(&state.web_directory),
        image_cache_folder: state.system_storage.folder(&state.image_cache_directory),
        cache_folder: state.system_storage.folder(&state.cache_directory),
        log_folder: state.system_storage.folder(state.system_logs.directory()),
        internal_metadata_folder: state
            .system_storage
            .folder(&state.internal_metadata_directory),
        transcoding_temp_folder: state.system_storage.folder(&state.transcode_directory),
        libraries,
    }))
}

pub(crate) async fn info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<SystemInfo>, ApiError> {
    Ok(Json(system_info(&state, &headers, &uri).await?))
}

pub(crate) async fn system_info(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<SystemInfo, ApiError> {
    authorization::require_first_time_setup_or_ignore_parental_control(state, headers, uri).await?;
    let public_info = public_system_info(state).await?;

    Ok(SystemInfo {
        web_socket_port_number: web_socket_port_number(&public_info),
        supports_library_monitor: true,
        completed_installations: Vec::new(),
        can_self_restart: true,
        can_launch_web_browser: false,
        program_data_path: path_string(&state.program_data_directory),
        web_path: path_string(&state.web_directory),
        items_by_name_path: path_string(&state.internal_metadata_directory),
        cache_path: path_string(&state.cache_directory),
        log_path: path_string(state.system_logs.directory()),
        internal_metadata_path: path_string(&state.internal_metadata_directory),
        transcoding_temp_path: path_string(&state.transcode_directory),
        cast_receiver_applications: Vec::new(),
        encoder_location: "System".to_owned(),
        system_architecture: "X64".to_owned(),
        public_info,
        ..SystemInfo::default()
    })
}

pub(crate) async fn public_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicSystemInfo>, ApiError> {
    Ok(Json(public_system_info(&state).await?))
}

pub(crate) async fn ping(State(state): State<Arc<AppState>>) -> Result<Response<Body>, ApiError> {
    let server_name = public_system_info(&state)
        .await?
        .server_name
        .unwrap_or_default();
    Response::builder()
        .header(header::CONTENT_TYPE, TEXT_UTF8)
        .body(Body::from(server_name))
        .map_err(|_| ApiError::Internal)
}

pub(crate) async fn endpoint_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    request: Request,
) -> Result<Json<EndPointInfo>, ApiError> {
    authorization::require_default(&state, request.headers(), &uri).await?;
    let connect_info = request.extensions().get::<ConnectInfo<SocketAddr>>();
    let remote_ip = remote_ip(connect_info);
    Ok(Json(EndPointInfo {
        is_local: is_local_request(connect_info),
        is_in_network: state.network_manager.is_in_local_network(remote_ip),
    }))
}

/// Compatibility response for the authenticated Infuse/plugin domain probe.
///
/// This is not part of Jellyfin's public API. An empty array is preferable to
/// inventing externally reachable hostnames the server cannot validate.
pub(crate) async fn server_domains() -> Json<Vec<String>> {
    Json(Vec::new())
}

pub(crate) async fn restart(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    request: Request,
) -> Result<StatusCode, ApiError> {
    require_local_access_or_elevated(
        &state,
        request.headers(),
        &uri,
        request.extensions().get::<ConnectInfo<SocketAddr>>(),
    )
    .await?;
    (state.system_command)(SystemCommand::Restart);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn shutdown(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    (state.system_command)(SystemCommand::Shutdown);
    Ok(StatusCode::NO_CONTENT)
}

impl From<SystemLogFile> for LogFileDto {
    fn from(log: SystemLogFile) -> Self {
        Self {
            date_created: log.date_created,
            date_modified: log.date_modified,
            size: log.size,
            name: log.name,
        }
    }
}

fn remote_ip(connect_info: Option<&ConnectInfo<SocketAddr>>) -> IpAddr {
    connect_info.map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |connect_info| {
        normalize_ip(connect_info.0.ip())
    })
}

fn is_local_request(connect_info: Option<&ConnectInfo<SocketAddr>>) -> bool {
    connect_info.is_none_or(|connect_info| normalize_ip(connect_info.0.ip()).is_loopback())
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address @ IpAddr::V4(_) => address,
    }
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) async fn public_system_info(state: &AppState) -> Result<PublicSystemInfo, ApiError> {
    let startup = startup::snapshot(state).await?;
    let mut public_info = state.system_info.clone();
    public_info
        .server_name
        .clone_from(&startup.configuration.server_name);
    public_info.startup_wizard_completed = Some(startup.completed);
    Ok(public_info)
}

fn web_socket_port_number(system_info: &PublicSystemInfo) -> i32 {
    system_info
        .local_address
        .as_deref()
        .and_then(|address| address.parse::<axum::http::Uri>().ok())
        .and_then(|uri| uri.port_u16())
        .map_or(8096, i32::from)
}

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}

async fn require_local_access_or_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    connect_info: Option<&ConnectInfo<SocketAddr>>,
) -> Result<(), ApiError> {
    if state
        .network_manager
        .is_in_local_network(remote_ip(connect_info))
    {
        return Ok(());
    }

    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::fs;
    use uuid::Uuid;

    use super::read_log_lines;

    #[tokio::test]
    async fn log_lines_page_without_buffering_the_whole_result() {
        let path = temp_path();
        fs::write(&path, b"first\r\nsecond\nthird\n").await.unwrap();

        let page = read_log_lines(fs::File::open(&path).await.unwrap(), -1, Some(1))
            .await
            .unwrap();
        assert_eq!(page, (vec!["first".to_owned()], 3));

        let page = read_log_lines(fs::File::open(&path).await.unwrap(), 1, Some(-1))
            .await
            .unwrap();
        assert_eq!(page, (vec!["second".to_owned(), "third".to_owned()], 3));
        fs::remove_file(path).await.unwrap();
    }

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("jellyfin-system-lines-{}", Uuid::new_v4().simple()))
    }
}
