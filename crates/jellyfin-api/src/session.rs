use std::{collections::HashMap, fmt::Write as _, sync::Arc};

use axum::{
    Json,
    extract::{
        OriginalUri, Path, Query, State, rejection::JsonRejection, rejection::PathRejection,
        rejection::QueryRejection,
    },
    http::{HeaderMap, StatusCode},
};
use chrono::{Duration, Utc};
use jellyfin_data::{DeviceQuery, NewActivityLog, NewSessionCommand, entities::device};
use jellyfin_model::{
    ClientCapabilitiesDto, GeneralCommand, GeneralCommandType, MediaType, MessageCommand,
    NameIdPair, PlayCommand, PlayRequest, PlayerStateInfo, PlaystateCommand, PlaystateRequest,
    SessionInfoDto, SessionUserInfo,
};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SessionQuery {
    controllable_by_user_id: Option<Uuid>,
    device_id: Option<String>,
    active_within_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CapabilitiesQuery {
    id: Option<String>,
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    playable_media_types: Vec<MediaType>,
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    supported_commands: Vec<GeneralCommandType>,
    supports_media_control: bool,
    supports_persistent_identifier: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ViewingQuery {
    #[serde(rename = "itemType")]
    ty: Option<String>,
    #[serde(rename = "itemId")]
    id: Option<String>,
    #[serde(rename = "itemName")]
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ReportViewingQuery {
    session_id: Option<String>,
    item_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PlayCommandQuery {
    play_command: Option<PlayCommand>,
    #[serde(default, deserialize_with = "crate::query::comma::deserialize")]
    item_ids: Vec<Uuid>,
    start_position_ticks: Option<i64>,
    media_source_id: Option<String>,
    audio_stream_index: Option<i32>,
    subtitle_stream_index: Option<i32>,
    start_index: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PlaystateCommandQuery {
    seek_position_ticks: Option<i64>,
}

impl Default for CapabilitiesQuery {
    fn default() -> Self {
        Self {
            id: None,
            playable_media_types: Vec::new(),
            supported_commands: Vec::new(),
            supports_media_control: false,
            supports_persistent_identifier: true,
        }
    }
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<SessionQuery>, QueryRejection>,
) -> Result<Json<Vec<SessionInfoDto>>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let device_query = DeviceQuery {
        device_id: query.device_id.filter(|device_id| !device_id.is_empty()),
        is_active: Some(true),
        active_since: query
            .active_within_seconds
            .filter(|seconds| *seconds > 0)
            .map(|seconds| Utc::now() - Duration::seconds(seconds)),
        ..DeviceQuery::default()
    };
    let page = state.devices.query(&device_query).await?;
    let is_api_key = matches!(identity, authentication::AuthenticatedIdentity::ApiKey(_));
    let authenticated_user = match &identity {
        authentication::AuthenticatedIdentity::Device(session) => Some(&session.user),
        authentication::AuthenticatedIdentity::ApiKey(_) => None,
    };
    let is_admin = is_api_key || authenticated_user.is_some_and(|user| user.is_administrator);
    let mut sessions = Vec::with_capacity(page.items.len());
    for device in page.items {
        let user = state.users.get(device.user_id).await?;
        let session_id = jellyfin_session_id(&device.app_name, &device.device_id);
        let connected = state.web_sockets.is_connected(&session_id).await;
        sessions.push(session_info(
            device,
            user.username,
            state.server_id(),
            connected,
        ));
    }
    let mut sessions = filter_controllable_sessions(
        sessions,
        authenticated_user,
        is_api_key,
        is_admin,
        query.controllable_by_user_id,
        &state,
    )
    .await?;
    if let authentication::AuthenticatedIdentity::Device(session) = &identity
        && !session.user.is_administrator
    {
        let viewer_id = session.user.id;
        sessions.retain(|session| {
            session.user_id == viewer_id
                || session
                    .additional_users
                    .iter()
                    .any(|additional| additional.user_id == viewer_id)
        });
    }
    Ok(Json(sessions))
}

pub(crate) async fn all_session_infos(state: &AppState) -> Result<Vec<SessionInfoDto>, ApiError> {
    let page = state
        .devices
        .query(&DeviceQuery {
            is_active: Some(true),
            ..DeviceQuery::default()
        })
        .await?;
    let mut sessions = Vec::with_capacity(page.items.len());
    for device in page.items {
        let user = state.users.get(device.user_id).await?;
        let session_id = jellyfin_session_id(&device.app_name, &device.device_id);
        let connected = state.web_sockets.is_connected(&session_id).await;
        sessions.push(session_info(
            device,
            user.username,
            state.server_id(),
            connected,
        ));
    }
    Ok(sessions)
}

#[allow(clippy::too_many_arguments)]
async fn filter_controllable_sessions(
    mut sessions: Vec<SessionInfoDto>,
    authenticated_user: Option<&jellyfin_data::entities::user::Model>,
    is_api_key: bool,
    is_admin: bool,
    controllable_by_user_id: Option<Uuid>,
    state: &AppState,
) -> Result<Vec<SessionInfoDto>, ApiError> {
    let policy = authenticated_user
        .map(authentication::stored_user_policy)
        .transpose()?;
    let can_control_others = is_api_key
        || policy
            .as_ref()
            .is_some_and(|policy| policy.enable_remote_control_of_other_users);
    if !is_api_key && let Some(user) = authenticated_user {
        sessions.retain(|session| session.user_id == user.id || can_control_others);
    }
    if let Some(controllable_user_id) = controllable_by_user_id {
        let controlled_user = state
            .users
            .get(controllable_user_id)
            .await
            .ok()
            .filter(|user| !user.is_hidden && !user.is_disabled);
        let Some(controlled_user) = controlled_user else {
            return Ok(Vec::new());
        };
        let controlled_policy = authentication::stored_user_policy(&controlled_user)?;
        sessions.retain(|session| session.supports_remote_control);
        if is_admin {
            return Ok(sessions);
        }
        if !controlled_policy.enable_shared_device_control {
            sessions.retain(|session| !session.user_id.is_nil());
        }
        if !can_control_others {
            let viewer_id = authenticated_user.map(|user| user.id);
            sessions.retain(|session| {
                session.user_id == Uuid::nil()
                    || viewer_id.is_some_and(|viewer_id| session.user_id == viewer_id)
                    || session
                        .additional_users
                        .iter()
                        .any(|additional| Some(additional.user_id) == viewer_id)
            });
        }
    }
    Ok(sessions)
}

pub(crate) async fn authentication_providers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.users.authentication_providers()))
}

pub(crate) async fn send_system_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, GeneralCommandType)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let Path((session_id, command)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let controller = authenticated_device_session(&state, &headers, &uri).await?;
    enforce_remote_control(&state, &controller, &session_id).await?;
    enqueue_general_command(
        &state,
        &session_id,
        &controller,
        GeneralCommand {
            name: command,
            controlling_user_id: controller.user.id,
            arguments: HashMap::new(),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn display_content(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<ViewingQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let item_type = required_query_value(query.ty)?;
    let item_id = required_query_value(query.id)?;
    let item_name = required_query_value(query.name)?;
    let controller = authenticated_device_session(&state, &headers, &uri).await?;
    enforce_remote_control(&state, &controller, &session_id).await?;
    enqueue_general_command(
        &state,
        &session_id,
        &controller,
        GeneralCommand {
            name: GeneralCommandType::DisplayContent,
            controlling_user_id: controller.user.id,
            arguments: HashMap::from([
                ("ItemId".to_owned(), item_id),
                ("ItemName".to_owned(), item_name),
                ("ItemType".to_owned(), item_type),
            ]),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn report_viewing(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<ReportViewingQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let session_id = if let Some(session_id) = query.session_id.filter(|value| !value.is_empty()) {
        session_id
    } else {
        let authentication::AuthenticatedIdentity::Device(session) = identity else {
            return Err(ApiError::Unauthorized);
        };
        jellyfin_session_id(&session.device.app_name, &session.device.device_id)
    };
    let item_id = required_query_value(query.item_id)?;
    let item_id = Uuid::parse_str(&item_id).map_err(|_| ApiError::InvalidRequest)?;
    let item = state
        .base_items
        .get(item_id)
        .await?
        .ok_or(jellyfin_data::BaseItemError::NotFound)?;
    let item = user_library::item_to_dto(item, state.server_id());
    let payload = serde_json::to_value(item).map_err(|_| ApiError::Internal)?;
    let device = find_active_session(&state, &session_id).await?;
    if state
        .devices
        .update_now_viewing_item(device.id, Some(payload))
        .await?
        != 1
    {
        return Err(ApiError::SessionNotFound);
    }
    crate::websocket::broadcast_sessions(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn send_general_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, GeneralCommandType)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    send_system_command(State(state), OriginalUri(uri), headers, path).await
}

pub(crate) async fn send_full_general_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    request: Result<Json<GeneralCommand>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let controller = authenticated_device_session(&state, &headers, &uri).await?;
    enforce_remote_control(&state, &controller, &session_id).await?;
    let Json(mut command) = request.map_err(|_| ApiError::InvalidRequest)?;
    command.controlling_user_id = controller.user.id;
    enqueue_general_command(&state, &session_id, &controller, command).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn send_message_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    request: Result<Json<MessageCommand>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let controller = authenticated_device_session(&state, &headers, &uri).await?;
    enforce_remote_control(&state, &controller, &session_id).await?;
    let Json(command) = request.map_err(|_| ApiError::InvalidRequest)?;
    let text = command
        .text
        .filter(|text| !text.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    let header = command
        .header
        .filter(|header| !header.trim().is_empty())
        .unwrap_or_else(|| "Message from Server".to_owned());
    let mut arguments = HashMap::from([("Header".to_owned(), header), ("Text".to_owned(), text)]);
    if let Some(timeout_ms) = command.timeout_ms {
        arguments.insert("TimeoutMs".to_owned(), timeout_ms.to_string());
    }
    enqueue_general_command(
        &state,
        &session_id,
        &controller,
        GeneralCommand {
            name: GeneralCommandType::DisplayMessage,
            controlling_user_id: controller.user.id,
            arguments,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn send_play_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PlayCommandQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let play_command = query.play_command.ok_or(ApiError::InvalidRequest)?;
    if query.item_ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }

    let controller = authenticated_device_session(&state, &headers, &uri).await?;
    enforce_remote_control(&state, &controller, &session_id).await?;
    enqueue_session_command(
        &state,
        &session_id,
        &controller,
        "Play",
        PlayRequest {
            item_ids: query.item_ids,
            start_position_ticks: query.start_position_ticks,
            play_command,
            controlling_user_id: controller.user.id,
            subtitle_stream_index: query.subtitle_stream_index,
            audio_stream_index: query.audio_stream_index,
            media_source_id: query.media_source_id,
            start_index: query.start_index,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn send_playstate_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, PlaystateCommand)>, PathRejection>,
    query: Result<Query<PlaystateCommandQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let Path((session_id, command)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let controller = authenticated_device_session(&state, &headers, &uri).await?;
    enforce_remote_control(&state, &controller, &session_id).await?;
    enqueue_session_command(
        &state,
        &session_id,
        &controller,
        "Playstate",
        PlaystateRequest {
            command,
            seek_position_ticks: query.seek_position_ticks,
            controlling_user_id: Some(controller.user.id.simple().to_string()),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn add_user_to_session(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Path((session_id, user_id)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let session = find_active_session(&state, &session_id).await?;
    if session.user_id == user_id {
        return Err(ApiError::InvalidRequest);
    }
    let user = state.users.get(user_id).await?;
    if state
        .devices
        .add_additional_user(session.id, user.id, &user.username)
        .await?
        != 1
    {
        return Err(ApiError::SessionNotFound);
    }
    crate::websocket::broadcast_sessions(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_user_from_session(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Path((session_id, user_id)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let session = find_active_session(&state, &session_id).await?;
    if session.user_id == user_id {
        return Err(ApiError::InvalidRequest);
    }
    if state
        .devices
        .remove_additional_user(session.id, user_id)
        .await?
        != 1
    {
        return Err(ApiError::SessionNotFound);
    }
    crate::websocket::broadcast_sessions(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn post_capabilities(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<CapabilitiesQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let access_token = current_session_access_token(&identity, query.id.as_deref())?;
    let capabilities = ClientCapabilitiesDto {
        playable_media_types: query.playable_media_types,
        supported_commands: query.supported_commands,
        supports_media_control: query.supports_media_control,
        supports_persistent_identifier: query.supports_persistent_identifier,
        ..ClientCapabilitiesDto::default()
    };
    persist_capabilities(&state, access_token, capabilities).await?;
    crate::websocket::broadcast_sessions(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn post_full_capabilities(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<CapabilitiesQuery>, QueryRejection>,
    request: Result<Json<ClientCapabilitiesDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let Json(capabilities) = request.map_err(|_| ApiError::InvalidRequest)?;
    let access_token = current_session_access_token(&identity, query.id.as_deref())?;
    persist_capabilities(&state, access_token, capabilities).await?;
    crate::websocket::broadcast_sessions(&state).await;
    Ok(StatusCode::NO_CONTENT)
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
    if let authentication::AuthenticatedIdentity::Device(session) = &identity {
        authentication::log_activity(
            &state,
            NewActivityLog::new(
                format!(
                    "{} is offline from {}",
                    session.user.username, session.device.device_name
                ),
                "SessionEnded",
                session.user.id,
            ),
        );
    }
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

async fn authenticated_device_session(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<Box<authentication::AuthenticatedSession>, ApiError> {
    match authentication::authenticated_identity(state, headers, Some(uri)).await? {
        authentication::AuthenticatedIdentity::Device(session) => Ok(session),
        authentication::AuthenticatedIdentity::ApiKey(_) => Err(ApiError::Unauthorized),
    }
}

fn required_query_value(value: Option<String>) -> Result<String, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(ApiError::InvalidRequest)
}

async fn enforce_remote_control(
    state: &AppState,
    controller: &authentication::AuthenticatedSession,
    target_session_id: &str,
) -> Result<(), ApiError> {
    if controller.user.is_administrator
        || authentication::stored_user_policy(&controller.user)?
            .enable_remote_control_of_other_users
    {
        return Ok(());
    }
    let target = find_active_session(state, target_session_id).await?;
    let target_user = state.users.get(target.user_id).await?;
    let target_shares =
        authentication::stored_user_policy(&target_user)?.enable_shared_device_control;
    let additional_users: Vec<SessionUserInfo> =
        serde_json::from_value(target.additional_users.clone()).unwrap_or_default();
    if target.user_id == controller.user.id
        || target_shares
            && additional_users
                .iter()
                .any(|additional| additional.user_id == controller.user.id)
    {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

async fn enqueue_general_command(
    state: &AppState,
    target_session_id: &str,
    controller: &authentication::AuthenticatedSession,
    command: GeneralCommand,
) -> Result<(), ApiError> {
    enqueue_session_command(
        state,
        target_session_id,
        controller,
        "GeneralCommand",
        command,
    )
    .await
}

async fn enqueue_session_command<T>(
    state: &AppState,
    target_session_id: &str,
    controller: &authentication::AuthenticatedSession,
    message_type: &str,
    payload: T,
) -> Result<(), ApiError>
where
    T: Serialize,
{
    find_active_session(state, target_session_id).await?;
    let queued = state
        .session_commands
        .enqueue(NewSessionCommand {
            target_session_id: target_session_id.to_owned(),
            controlling_session_id: Some(jellyfin_session_id(
                &controller.device.app_name,
                &controller.device.device_id,
            )),
            message_type: message_type.to_owned(),
            payload: serde_json::to_value(payload).map_err(|_| ApiError::Internal)?,
        })
        .await?;
    // Deliver immediately when connected; otherwise the row stays for replay.
    if state
        .web_sockets
        .send_command(target_session_id, message_type, &queued.payload)
        .await
    {
        let _ = state.session_commands.delete(&[queued.id]).await;
    }
    Ok(())
}

async fn find_active_session(
    state: &AppState,
    session_id: &str,
) -> Result<device::Model, ApiError> {
    if session_id.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    let sessions = state
        .devices
        .query(&DeviceQuery {
            is_active: Some(true),
            ..DeviceQuery::default()
        })
        .await?;
    sessions
        .items
        .into_iter()
        .find(|device| jellyfin_session_id(&device.app_name, &device.device_id) == session_id)
        .ok_or(ApiError::SessionNotFound)
}

fn session_info(
    device: device::Model,
    user_name: String,
    server_id: &str,
    has_web_socket: bool,
) -> SessionInfoDto {
    let capabilities: ClientCapabilitiesDto =
        serde_json::from_value(device.capabilities).unwrap_or_default();
    let play_state: PlayerStateInfo = serde_json::from_value(device.play_state).unwrap_or_default();
    let additional_users: Vec<SessionUserInfo> =
        serde_json::from_value(device.additional_users).unwrap_or_default();
    let now_playing_queue: Vec<serde_json::Value> =
        serde_json::from_value(device.now_playing_queue).unwrap_or_default();
    let playable_media_types = capabilities.playable_media_types.clone();
    let supported_commands = capabilities.supported_commands.clone();
    SessionInfoDto {
        play_state,
        additional_users,
        id: Some(jellyfin_session_id(&device.app_name, &device.device_id)),
        user_id: device.user_id,
        user_name: Some(user_name),
        client: Some(device.app_name),
        last_activity_date: device.date_last_activity,
        last_playback_check_in: device.date_last_activity,
        last_paused_date: device.date_last_paused,
        device_name: Some(device.device_name),
        device_type: None,
        now_playing_item: device.now_playing_item,
        device_id: Some(device.device_id),
        application_version: Some(device.app_version),
        is_active: device.is_active,
        supports_media_control: capabilities.supports_media_control && has_web_socket,
        supports_remote_control: capabilities.supports_media_control && has_web_socket,
        now_playing_queue,
        has_custom_device_name: false,
        playlist_item_id: device.playlist_item_id,
        server_id: Some(server_id.to_owned()),
        user_primary_image_tag: None,
        now_viewing_item: device.now_viewing_item,
        capabilities,
        playable_media_types,
        supported_commands,
    }
}

async fn persist_capabilities(
    state: &AppState,
    access_token: &str,
    capabilities: ClientCapabilitiesDto,
) -> Result<(), ApiError> {
    let capabilities = serde_json::to_value(capabilities).map_err(|_| ApiError::Internal)?;
    if state
        .devices
        .update_capabilities_by_token(access_token, capabilities)
        .await?
        != 1
    {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

fn current_session_access_token<'a>(
    identity: &'a authentication::AuthenticatedIdentity,
    requested_id: Option<&str>,
) -> Result<&'a str, ApiError> {
    let authentication::AuthenticatedIdentity::Device(session) = identity else {
        return Err(ApiError::Unauthorized);
    };
    let requested_id = requested_id.filter(|value| !value.trim().is_empty());
    if requested_id.is_some_and(|id| {
        id != session.device.id.to_string()
            && id != jellyfin_session_id(&session.device.app_name, &session.device.device_id)
    }) {
        return Err(ApiError::InvalidRequest);
    }
    Ok(&session.access_token)
}

pub(crate) fn jellyfin_session_id(app_name: &str, device_id: &str) -> String {
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
