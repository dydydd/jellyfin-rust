use std::{fmt::Write as _, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
};
use chrono::{Duration, Utc};
use jellyfin_data::{DeviceQuery, entities::device};
use jellyfin_model::{ClientCapabilitiesDto, NameIdPair, SessionInfoDto};
use md5::{Digest, Md5};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SessionQuery {
    controllable_by_user_id: Option<Uuid>,
    device_id: Option<String>,
    active_within_seconds: Option<i64>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<SessionQuery>, QueryRejection>,
) -> Result<Json<Vec<SessionInfoDto>>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    if query.controllable_by_user_id.is_some() {
        return Ok(Json(Vec::new()));
    }

    let mut device_query = DeviceQuery {
        device_id: query.device_id.filter(|device_id| !device_id.is_empty()),
        is_active: Some(true),
        active_since: query
            .active_within_seconds
            .filter(|seconds| *seconds > 0)
            .map(|seconds| Utc::now() - Duration::seconds(seconds)),
        ..DeviceQuery::default()
    };
    if let authentication::AuthenticatedIdentity::Device(session) = &identity
        && !session.user.is_administrator
    {
        device_query.user_id = Some(session.user.id);
    }

    let page = state.devices.query(&device_query).await?;
    let mut sessions = Vec::with_capacity(page.items.len());
    for device in page.items {
        let user = state.users.get(device.user_id).await?;
        sessions.push(session_info(device, user.username, state.server_id()));
    }
    Ok(Json(sessions))
}

pub(crate) async fn authentication_providers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.users.authentication_providers()))
}

pub(crate) async fn password_reset_providers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.users.password_reset_providers()))
}

pub(crate) async fn logout(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    state
        .devices
        .delete_by_token(identity.access_token())
        .await?;
    Ok(StatusCode::NO_CONTENT)
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

fn session_info(device: device::Model, user_name: String, server_id: &str) -> SessionInfoDto {
    SessionInfoDto {
        id: Some(jellyfin_session_id(&device.app_name, &device.device_id)),
        user_id: device.user_id,
        user_name: Some(user_name),
        client: Some(device.app_name),
        last_activity_date: device.date_last_activity,
        last_playback_check_in: device.date_last_activity,
        last_paused_date: None,
        device_name: Some(device.device_name),
        device_type: None,
        device_id: Some(device.device_id),
        application_version: Some(device.app_version),
        is_active: device.is_active,
        supports_media_control: false,
        supports_remote_control: false,
        has_custom_device_name: false,
        playlist_item_id: None,
        server_id: Some(server_id.to_owned()),
        user_primary_image_tag: None,
        capabilities: ClientCapabilitiesDto::default(),
        playable_media_types: Vec::new(),
        supported_commands: Vec::new(),
    }
}

fn jellyfin_session_id(app_name: &str, device_id: &str) -> String {
    let key = format!("{app_name}{device_id}");
    let mut hasher = Md5::new();
    for unit in key.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    let digest = hasher.finalize();
    let bytes = digest.as_slice();
    let mut result = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0], bytes[5], bytes[4], bytes[7], bytes[6]
    );
    for byte in &bytes[8..] {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}
