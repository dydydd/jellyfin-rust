use std::{cmp::Reverse, sync::Arc};

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::Query;
use jellyfin_data::{DeviceQuery, entities::device};
use jellyfin_model::{ClientCapabilitiesDto, DeviceInfoDto, QueryResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct DevicesQuery {
    user_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct DeviceIdQuery {
    id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct DeleteDevicesQuery {
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    id: Vec<String>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<DevicesQuery>,
) -> Result<Json<QueryResult<DeviceInfoDto>>, ApiError> {
    let target_user_id = elevated_target_user_id(&state, &headers, &uri, query.user_id).await?;
    let device_query = DeviceQuery {
        user_id: target_user_id.filter(|user_id| !user_id.is_nil()),
        ..DeviceQuery::default()
    };
    let mut devices = state.devices.query(&device_query).await?.items;
    devices.sort_by_key(|device| (Reverse(device.date_last_activity), device.device_id.clone()));

    let mut items = Vec::with_capacity(devices.len());
    for device in devices {
        items.push(device_info(&state, device).await?);
    }
    Ok(Json(QueryResult::from_items(items)))
}

pub(crate) async fn info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<DeviceIdQuery>,
) -> Result<Json<DeviceInfoDto>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let id = query
        .id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let device = state
        .devices
        .latest_by_device_id(id)
        .await?
        .ok_or(ApiError::DeviceNotFound)?;
    Ok(Json(device_info(&state, device).await?))
}

pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<DeleteDevicesQuery>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    for id in &query.id {
        if id.is_empty() || state.devices.latest_by_device_id(id).await?.is_none() {
            return Err(ApiError::InvalidRequest);
        }
    }
    for id in query.id {
        state.devices.delete_by_device_id(&id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn elevated_target_user_id(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    requested_user_id: Option<Uuid>,
) -> Result<Option<Uuid>, ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.require_administrator()?;
    Ok(match identity {
        authentication::AuthenticatedIdentity::Device(session) => {
            Some(identity_target_user_id(&session.user, requested_user_id)?)
        }
        authentication::AuthenticatedIdentity::ApiKey(_) => requested_user_id,
    })
}

fn identity_target_user_id(
    user: &jellyfin_data::entities::user::Model,
    requested_user_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let requested_user_id = requested_user_id.filter(|user_id| !user_id.is_nil());
    if let Some(user_id) = requested_user_id {
        if user_id != user.id && !user.is_administrator {
            return Err(ApiError::Forbidden);
        }
        Ok(user_id)
    } else {
        Ok(user.id)
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

async fn device_info(state: &AppState, device: device::Model) -> Result<DeviceInfoDto, ApiError> {
    let user = state.users.get(device.user_id).await?;
    let capabilities: ClientCapabilitiesDto =
        serde_json::from_value(device.capabilities).unwrap_or_default();
    let icon_url = capabilities.icon_url.clone();
    Ok(DeviceInfoDto {
        name: Some(device.device_name),
        custom_name: None,
        access_token: None,
        id: Some(device.device_id),
        last_user_name: Some(user.username),
        app_name: Some(device.app_name),
        app_version: Some(device.app_version),
        last_user_id: Some(device.user_id),
        date_last_activity: Some(device.date_last_activity),
        capabilities,
        icon_url,
    })
}
