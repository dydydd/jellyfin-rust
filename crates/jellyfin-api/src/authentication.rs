use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, header},
};
use jellyfin_data::{NewDevice, entities::user};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ApiError, AppState, user_to_dto};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AuthenticateUserByName {
    pub username: Option<String>,
    pub pw: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    user: jellyfin_model::UserDto,
    session_info: Value,
    access_token: String,
    server_id: String,
}

pub(crate) async fn authenticate_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<AuthenticateUserByName>, JsonRejection>,
) -> Result<Json<AuthenticationResult>, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let username = request.username.unwrap_or_default();
    let password = request.pw.unwrap_or_default();
    let client = ClientMetadata::from_headers(&headers)?;

    let mut user = state
        .users
        .get_by_name(&username)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if user.is_disabled {
        return Err(ApiError::Forbidden);
    }
    let authentication = state.authentication;
    let user = tokio::task::spawn_blocking(move || {
        authentication.authenticate(&username, &password, Some(&mut user))?;
        Ok::<_, jellyfin_server_implementations::AuthenticationError>(user)
    })
    .await
    .map_err(|_| ApiError::Internal)??;
    let user = state.users.record_successful_authentication(&user).await?;
    let session = state
        .devices
        .create_session(NewDevice::new(
            user.id,
            &client.client,
            &client.version,
            &client.device,
            &client.device_id,
        ))
        .await?;

    let mut user_dto = user_to_dto(user.clone());
    user_dto.server_id = Some(state.server_id().to_owned());
    Ok(Json(AuthenticationResult {
        user: user_dto,
        session_info: json!({
            "Id": session.id.to_string(),
            "UserId": user.id.simple().to_string(),
            "UserName": user.username,
            "Client": session.app_name,
            "ApplicationVersion": session.app_version,
            "DeviceName": session.device_name,
            "DeviceId": session.device_id,
            "IsActive": session.is_active,
        }),
        access_token: session.access_token,
        server_id: state.server_id().to_owned(),
    }))
}

pub(crate) async fn current_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<jellyfin_model::UserDto>, ApiError> {
    let authenticated = authenticated_session(&state, &headers).await?;
    let mut dto = user_to_dto(authenticated.user);
    dto.server_id = Some(state.server_id().to_owned());
    Ok(Json(dto))
}

pub(crate) struct AuthenticatedSession {
    pub(crate) user: user::Model,
    pub(crate) access_token: String,
}

pub(crate) async fn authenticated_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AuthenticatedSession, ApiError> {
    let access_token = access_token(headers).ok_or(ApiError::Unauthorized)?;
    let session = state
        .devices
        .find_by_token(&access_token)
        .await?
        .filter(|session| session.is_active)
        .ok_or(ApiError::Unauthorized)?;
    let user = state.users.get(session.user_id).await?;
    if user.is_disabled {
        return Err(ApiError::Forbidden);
    }
    Ok(AuthenticatedSession { user, access_token })
}

#[derive(Debug, Default)]
struct ClientMetadata {
    client: String,
    device_id: String,
    device: String,
    version: String,
    token: Option<String>,
}

impl ClientMetadata {
    fn from_headers(headers: &HeaderMap) -> Result<Self, ApiError> {
        let header = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(ApiError::InvalidRequest)?;
        let metadata = parse_authorization(header);
        if metadata.device_id.is_empty() {
            return Err(ApiError::InvalidRequest);
        }
        Ok(metadata)
    }
}

fn access_token(headers: &HeaderMap) -> Option<String> {
    for name in ["x-emby-token", "x-mediabrowser-token"] {
        if let Some(token) = headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
        {
            return Some(token.to_owned());
        }
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_authorization(value).token)
}

fn parse_authorization(value: &str) -> ClientMetadata {
    let fields = value.split_once(' ').map_or(value, |(_, fields)| fields);
    let mut metadata = ClientMetadata::default();
    for field in fields.split(',') {
        let Some((key, value)) = field.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let decoded = percent_decode_str(value).decode_utf8_lossy().into_owned();
        if key.eq_ignore_ascii_case("Client") {
            metadata.client = decoded;
        } else if key.eq_ignore_ascii_case("DeviceId") {
            metadata.device_id = decoded;
        } else if key.eq_ignore_ascii_case("Device") {
            metadata.device = decoded;
        } else if key.eq_ignore_ascii_case("Version") {
            metadata.version = decoded;
        } else if key.eq_ignore_ascii_case("Token") && !decoded.is_empty() {
            metadata.token = Some(decoded);
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_media_browser_header() {
        let metadata = parse_authorization(
            "MediaBrowser Client=\"Jellyfin.Server%20Integration%20Tests\", \
             DeviceId=\"69420\", Device=\"Apple%20II\", Version=\"10.8.0\", Token=\"abc\"",
        );
        assert_eq!(metadata.client, "Jellyfin.Server Integration Tests");
        assert_eq!(metadata.device_id, "69420");
        assert_eq!(metadata.device, "Apple II");
        assert_eq!(metadata.version, "10.8.0");
        assert_eq!(metadata.token.as_deref(), Some("abc"));
    }
}
