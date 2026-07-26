use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, OriginalUri, Query, Request, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, Response, StatusCode, header},
};
use chrono::{DateTime, Utc};
use jellyfin_controller::SystemLogFile;
use jellyfin_model::{
    EndPointInfo, LibraryStorageDto, PublicSystemInfo, SystemInfo, SystemStorageDto,
};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::{ApiError, AppState, authentication, authorization, startup};

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
    authorization::require_first_time_setup_or_ignore_parental_control(&state, &headers, &uri)
        .await?;
    let startup = startup::snapshot(&state).await?;
    let mut public_info = state.system_info.clone();
    public_info
        .server_name
        .clone_from(&startup.configuration.server_name);
    public_info.startup_wizard_completed = Some(startup.completed);

    Ok(Json(SystemInfo {
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
    }))
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
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn shutdown(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
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

fn web_socket_port_number(system_info: &PublicSystemInfo) -> i32 {
    system_info
        .local_address
        .as_deref()
        .and_then(|address| address.parse::<axum::http::Uri>().ok())
        .and_then(|uri| uri.port_u16())
        .map(i32::from)
        .unwrap_or(8096)
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
