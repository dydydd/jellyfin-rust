use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Json,
    body::Body,
    extract::{ConnectInfo, OriginalUri, Query, Request, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, Response, header},
};
use chrono::{DateTime, Utc};
use jellyfin_controller::SystemLogFile;
use jellyfin_model::{EndPointInfo, LibraryStorageDto, SystemStorageDto};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::{ApiError, AppState, authentication, authorization};

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

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}
