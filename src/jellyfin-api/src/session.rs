use std::{collections::HashMap, fmt::Write as _, sync::Arc};

use axum::{
    Json,
    extract::{
        FromRequest, OriginalUri, Path, Request, State, rejection::JsonRejection,
        rejection::PathRejection,
    },
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::{Query, QueryRejection};
use chrono::{Duration, Utc};
use jellyfin_data::{DeviceQuery, NewActivityLog, NewSessionCommand, entities::device};
use jellyfin_model::{
    ClientCapabilitiesDto, GeneralCommand, GeneralCommandType, MediaType, MessageCommand,
    NameIdPair, PlayCommand, PlayRequest, PlayerStateInfo, PlaystateCommand, PlaystateRequest,
    QueryResult, SessionInfoDto, SessionUserInfo, TranscodingInfo,
};
use md5::{Digest, Md5};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _, de::IgnoredAny};
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library, user_primary_image_tags};
use user_library::{BaseItemDto, BaseItemDtoFields};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SessionQuery {
    #[serde(alias = "ControllableByUserId", alias = "controllablebyuserid")]
    controllable_by_user_id: Option<Uuid>,
    #[serde(alias = "DeviceId", alias = "deviceid")]
    device_id: Option<String>,
    #[serde(alias = "ActiveWithinSeconds", alias = "activewithinseconds")]
    active_within_seconds: Option<i32>,
    #[serde(alias = "Id", alias = "id")]
    id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PlayQueueQuery {
    #[serde(alias = "Id", alias = "id")]
    id: Option<String>,
    #[serde(alias = "DeviceId", alias = "deviceid")]
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CapabilitiesQuery {
    #[serde(alias = "Id", alias = "ID")]
    id: Option<String>,
    #[serde(
        default,
        alias = "PlayableMediaTypes",
        alias = "playablemediatypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    playable_media_types: Vec<MediaType>,
    #[serde(
        default,
        alias = "SupportedCommands",
        alias = "supportedcommands",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    supported_commands: Vec<GeneralCommandType>,
    #[serde(alias = "SupportsMediaControl", alias = "supportsmediacontrol")]
    supports_media_control: bool,
    #[serde(
        alias = "SupportsPersistentIdentifier",
        alias = "supportspersistentidentifier"
    )]
    supports_persistent_identifier: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FullCapabilitiesQuery {
    #[serde(alias = "Id")]
    id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ViewingQuery {
    #[serde(rename = "itemType", alias = "ItemType", alias = "itemtype")]
    ty: Option<String>,
    #[serde(rename = "itemId", alias = "ItemId", alias = "itemid")]
    id: Option<String>,
    #[serde(rename = "itemName", alias = "ItemName", alias = "itemname")]
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ReportViewingQuery {
    #[serde(alias = "SessionId", alias = "sessionid")]
    session_id: Option<String>,
    #[serde(alias = "ItemId", alias = "itemid")]
    item_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PlayCommandQuery {
    #[serde(alias = "PlayCommand", alias = "playcommand")]
    play_command: Option<PlayCommand>,
    #[serde(
        default,
        alias = "ItemIds",
        alias = "itemids",
        deserialize_with = "crate::query::comma::deserialize_model_binder"
    )]
    item_ids: Vec<Uuid>,
    #[serde(alias = "StartPositionTicks", alias = "startpositionticks")]
    start_position_ticks: Option<i64>,
    #[serde(alias = "MediaSourceId", alias = "mediasourceid")]
    media_source_id: Option<String>,
    #[serde(alias = "AudioStreamIndex", alias = "audiostreamindex")]
    audio_stream_index: Option<i32>,
    #[serde(alias = "SubtitleStreamIndex", alias = "subtitlestreamindex")]
    subtitle_stream_index: Option<i32>,
    #[serde(alias = "StartIndex", alias = "startindex")]
    start_index: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PlaystateCommandQuery {
    #[serde(alias = "SeekPositionTicks", alias = "seekpositionticks")]
    seek_position_ticks: Option<i64>,
    #[serde(alias = "ControllingUserId", alias = "controllinguserid")]
    controlling_user_id: Option<String>,
}

impl Default for CapabilitiesQuery {
    fn default() -> Self {
        Self {
            id: None,
            playable_media_types: Vec::new(),
            supported_commands: Vec::new(),
            supports_media_control: false,
            // The capabilities query parameter defaults to true in the
            // official SessionController. This differs from the DTO's serde
            // default, which remains false when omitted from JSON.
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
    let (requester_id, requester_policy, is_admin, is_api_key) = match &identity {
        authentication::AuthenticatedIdentity::Device(session) => {
            let policy = authentication::stored_user_policy(&session.user)?;
            (
                session.user.id,
                Some(policy),
                session.user.is_administrator,
                false,
            )
        }
        authentication::AuthenticatedIdentity::ApiKey(_) => (Uuid::nil(), None, true, true),
    };
    // RequestHelpers treats an explicitly supplied empty Guid as an omitted
    // target and substitutes the authenticated user when one exists.
    let controllable_user_id = match query.controllable_by_user_id {
        Some(id) if id.is_nil() && !requester_id.is_nil() => Some(requester_id),
        Some(id) if id.is_nil() => None,
        value => value,
    };
    if let Some(target_user_id) = controllable_user_id {
        if !is_admin && requester_id != target_user_id {
            return Err(ApiError::Forbidden);
        }
    }

    let device_query = DeviceQuery {
        device_id: query.device_id.filter(|device_id| !device_id.is_empty()),
        is_active: Some(true),
        active_since: query
            .active_within_seconds
            .filter(|seconds| *seconds > 0)
            .map(|seconds| Utc::now() - Duration::seconds(i64::from(seconds))),
        ..DeviceQuery::default()
    };
    let page = state.devices.query(&device_query).await?;
    let mut sessions = Vec::with_capacity(page.items.len());
    let controllable_policy = if let Some(target_user_id) = controllable_user_id {
        let target_user = match state.users.get(target_user_id).await {
            Ok(user) => user,
            // Jellyfin treats an unknown controllable user as an empty
            // session result rather than surfacing a 404 from the user store.
            Err(jellyfin_controller::UserError::NotFound) => return Ok(Json(Vec::new())),
            Err(error) => return Err(error.into()),
        };
        Some(authentication::stored_user_policy(&target_user)?)
    } else {
        None
    };
    let requester_can_control_others = requester_policy
        .as_ref()
        .is_some_and(|policy| policy.enable_remote_control_of_other_users)
        || is_api_key;
    let user_details = session_user_details(&state, &page.items).await?;
    for device in page.items {
        if let Some(session_id) = query.id.as_deref().filter(|id| !id.is_empty())
            && jellyfin_session_id(&device.app_name, &device.device_id) != session_id
        {
            continue;
        }
        let connected = state
            .web_sockets
            .is_connected(&jellyfin_session_id(&device.app_name, &device.device_id))
            .await;
        if controllable_user_id.is_some() {
            let capabilities =
                ClientCapabilitiesDto::from_stored_value(device.capabilities.clone());
            if !capabilities.supports_media_control || !connected {
                continue;
            }
            if controllable_policy.as_ref().is_some_and(|p| {
                !controlled_user_allows_session(p.enable_shared_device_control, device.user_id)
            }) {
                continue;
            }
            if !requester_can_control_others
                && !can_control_session(
                    device.user_id,
                    &device.additional_users,
                    requester_id,
                    false,
                )
            {
                continue;
            }
            if !is_api_key
                && requester_policy.as_ref().is_some_and(|policy| {
                    !can_access_session_device(
                        policy,
                        &device.device_id,
                        capabilities.supports_persistent_identifier,
                    )
                })
            {
                continue;
            }
        } else if !is_admin
            && !can_control_session(
                device.user_id,
                &device.additional_users,
                requester_id,
                false,
            )
        {
            continue;
        }
        let (user_name, primary_image_tag) = user_details
            .get(&device.user_id)
            .map(|(name, tag)| (Some(name.clone()), tag.clone()))
            .unwrap_or((None, None));
        let mut transcoding_info = transcoding_info(&state, &device.device_id);
        adapt_emby_transcoding_info(&uri, &mut transcoding_info);
        sessions.push(session_info(
            device,
            user_name,
            primary_image_tag,
            state.server_id(),
            connected,
            transcoding_info,
        ));
    }
    Ok(Json(sessions))
}

fn can_access_session_device(
    policy: &jellyfin_model::UserPolicy,
    device_id: &str,
    supports_persistent_identifier: bool,
) -> bool {
    device_id.trim().is_empty()
        || policy.is_administrator
        || policy.enable_all_devices
        || policy
            .enabled_devices
            .iter()
            .any(|enabled| enabled.eq_ignore_ascii_case(device_id))
        || !supports_persistent_identifier
}

fn controlled_user_allows_session(enable_shared_device_control: bool, owner_id: Uuid) -> bool {
    enable_shared_device_control || !owner_id.is_nil()
}

/// Returns the queue stored on a live device session. Emby's PlayQueue route
/// is backed by the same session state as Jellyfin; queue entries are only
/// identifiers, so hydrate them through the normal user-aware DTO projector.
pub(crate) async fn play_queue(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<PlayQueueQuery>, QueryRejection>,
) -> Result<Json<QueryResult<BaseItemDto>>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let device = if let Some(id) = query.id.as_deref().filter(|id| !id.is_empty()) {
        find_active_session(&state, id).await?
    } else if let Some(device_id) = query
        .device_id
        .as_deref()
        .filter(|device_id| !device_id.is_empty())
    {
        state
            .devices
            .query(&DeviceQuery {
                device_id: Some(device_id.to_owned()),
                is_active: Some(true),
                ..DeviceQuery::default()
            })
            .await?
            .items
            .into_iter()
            .next()
            .ok_or(ApiError::SessionNotFound)?
    } else {
        match &identity {
            authentication::AuthenticatedIdentity::Device(session) => session.device.clone(),
            authentication::AuthenticatedIdentity::ApiKey(_) => {
                return Err(ApiError::InvalidRequest);
            }
        }
    };

    if let authentication::AuthenticatedIdentity::Device(session) = &identity
        && !session.user.is_administrator
        && device.user_id != session.user.id
        && !serde_json::from_value::<Vec<SessionUserInfo>>(device.additional_users.clone())
            .unwrap_or_default()
            .iter()
            .any(|user| user.user_id == session.user.id)
    {
        return Err(ApiError::Forbidden);
    }

    let queue = queue_item_ids(&device.now_playing_queue);
    let items = state.base_items.get_many(&queue).await?;
    let mut by_id = items
        .into_iter()
        .map(|item| (item.id, item))
        .collect::<HashMap<_, _>>();
    let mut projected = Vec::with_capacity(queue.len());
    for id in queue {
        if let Some(item) = by_id.remove(&id) {
            projected.push(
                user_library::project_item_to_dto(
                    &state,
                    item,
                    device.user_id,
                    BaseItemDtoFields::all(),
                    None,
                    None,
                )
                .await?,
            );
        }
    }
    user_library::omit_incompatible_emby_relations(&uri, &mut projected);
    Ok(Json(
        QueryResult::from_items(projected).map_err(|_| ApiError::Internal)?,
    ))
}

fn queue_item_ids(value: &serde_json::Value) -> Vec<Uuid> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            ["Id", "id", "ItemId", "itemId", "itemid"]
                .into_iter()
                .find_map(|key| entry.get(key).and_then(|value| value.as_str()))
                .and_then(|id| Uuid::parse_str(id).ok())
        })
        .collect()
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
    let user_details = session_user_details(state, &page.items).await?;
    for device in page.items {
        let (user_name, primary_image_tag) = user_details
            .get(&device.user_id)
            .map(|(name, tag)| (Some(name.clone()), tag.clone()))
            .unwrap_or((None, None));
        let connected = state
            .web_sockets
            .is_connected(&jellyfin_session_id(&device.app_name, &device.device_id))
            .await;
        let transcoding_info = transcoding_info(state, &device.device_id);
        sessions.push(session_info(
            device,
            user_name,
            primary_image_tag,
            state.server_id(),
            connected,
            transcoding_info,
        ));
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
    let controller = authenticated_session_controller(&state, &headers, &uri).await?;
    let Path((session_id, command)) = path.map_err(|_| ApiError::InvalidRequest)?;
    enqueue_general_command(
        &state,
        &session_id,
        &controller,
        GeneralCommand {
            name: command,
            controlling_user_id: controller.user_id(),
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
    let controller = authenticated_session_controller(&state, &headers, &uri).await?;
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let item_type = required_query_value(query.ty)?;
    let item_id = required_query_value(query.id)?;
    let item_name = required_query_value(query.name)?;
    enqueue_general_command(
        &state,
        &session_id,
        &controller,
        GeneralCommand {
            name: GeneralCommandType::DisplayContent,
            controlling_user_id: controller.user_id(),
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
        let authentication::AuthenticatedIdentity::Device(session) = &identity else {
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
    assert_identity_can_control_session(&device, &identity)?;
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
    let controller = authenticated_session_controller(&state, &headers, &uri).await?;
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Json(mut command) = request.map_err(|_| ApiError::InvalidRequest)?;
    command.controlling_user_id = controller.user_id();
    enqueue_general_command(&state, &session_id, &controller, command).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn send_message_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Result<StatusCode, ApiError> {
    let controller = authenticated_session_controller(&state, &headers, &uri).await?;
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let emby_protocol = uri.path() == "/emby" || uri.path().starts_with("/emby/");
    let command = if emby_protocol {
        emby_message_command(&uri)?
    } else {
        let Json(command) = Json::<MessageCommand>::from_request(request, &state)
            .await
            .map_err(|_| ApiError::InvalidRequest)?;
        command
    };
    let text = command
        .text
        .filter(|text| !text.trim().is_empty())
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
            controlling_user_id: controller.user_id(),
            arguments,
        },
    )
    .await?;
    Ok(if emby_protocol {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    })
}

fn emby_message_command(uri: &axum::http::Uri) -> Result<MessageCommand, ApiError> {
    let mut text = None;
    let mut header = None;
    let mut timeout_ms = None;
    let mut has_text = false;
    let mut has_header = false;

    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("Text") {
            has_text = true;
            text = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("Header") {
            has_header = true;
            header = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("TimeoutMs") {
            timeout_ms = Some(value.into_owned());
        }
    }
    if !has_text || !has_header {
        return Err(ApiError::InvalidRequest);
    }
    let timeout_ms = timeout_ms
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<i64>().map_err(|_| ApiError::InvalidRequest))
        .transpose()?;

    Ok(MessageCommand {
        header,
        text,
        timeout_ms,
    })
}

pub(crate) async fn send_play_command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<PlayCommandQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let controller = authenticated_session_controller(&state, &headers, &uri).await?;
    let Path(session_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let play_command = query.play_command.ok_or(ApiError::InvalidRequest)?;
    if query.item_ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }

    enqueue_session_command(
        &state,
        &session_id,
        &controller,
        "Play",
        PlayRequest {
            item_ids: query.item_ids,
            start_position_ticks: query.start_position_ticks,
            play_command,
            controlling_user_id: controller.user_id(),
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
    request: Request,
) -> Result<StatusCode, ApiError> {
    let controller = authenticated_session_controller(&state, &headers, &uri).await?;
    let Path((session_id, command)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let emby_protocol = uri.path() == "/emby" || uri.path().starts_with("/emby/");
    let (seek_position_ticks, controlling_user_id) = if emby_protocol {
        let Json(request) = Json::<EmbyPlaystateRequest>::from_request(request, &state)
            .await
            .map_err(|_| ApiError::InvalidRequest)?;
        // Both the generated body and route carry Command. As in Jellyfin's
        // SessionController, the required route value is authoritative.
        let _ = request.command;
        (request.seek_position_ticks, request.controlling_user_id)
    } else {
        let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
        (query.seek_position_ticks, query.controlling_user_id)
    };
    enqueue_session_command(
        &state,
        &session_id,
        &controller,
        "Playstate",
        PlaystateRequest {
            command,
            seek_position_ticks,
            // The official endpoint binds this as a nullable string; keep
            // caller-supplied values, including omission, unchanged on wire.
            controlling_user_id,
        },
    )
    .await?;
    Ok(if emby_protocol {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    })
}

/// Emby's generated legacy route binds a complete `PlaystateRequest` body,
/// unlike Jellyfin's query-based endpoint. A visitor is used instead of a
/// derived struct so ASP.NET-style case-insensitive properties retain the
/// last value when differently-cased duplicates are submitted.
#[derive(Debug, Default)]
struct EmbyPlaystateRequest {
    command: Option<PlaystateCommand>,
    seek_position_ticks: Option<i64>,
    controlling_user_id: Option<String>,
}

impl<'de> Deserialize<'de> for EmbyPlaystateRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RequestVisitor;

        impl<'de> serde::de::Visitor<'de> for RequestVisitor {
            type Value = EmbyPlaystateRequest;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an Emby PlaystateRequest object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut request = EmbyPlaystateRequest::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("Command") {
                        request.command = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("SeekPositionTicks") {
                        request.seek_position_ticks = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("ControllingUserId") {
                        request.controlling_user_id = map.next_value()?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(request)
            }
        }

        deserializer.deserialize_map(RequestVisitor)
    }
}

pub(crate) async fn add_user_to_session(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Path((session_id, user_id)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let session = find_active_session(&state, &session_id).await?;
    assert_identity_can_control_session(&session, &identity)?;
    assert_can_attach_user(&identity, user_id)?;
    if session.user_id == user_id {
        return Err(ApiError::InvalidRequest);
    }
    let user = match state.users.get(user_id).await {
        Ok(user) => user,
        // SessionManager exposes an unknown additional user as an argument
        // error after its control and attach assertions.
        Err(jellyfin_controller::UserError::NotFound) => return Err(ApiError::InvalidRequest),
        Err(error) => return Err(error.into()),
    };
    if state
        .devices
        .add_additional_user(session.id, user.id, &user.username)
        .await?
        != 1
    {
        return Err(ApiError::SessionNotFound);
    }
    state
        .web_sockets
        .add_session_user(&session_id, user.id)
        .await;
    crate::websocket::broadcast_sessions(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_user_from_session(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Path((session_id, user_id)) = path.map_err(|_| ApiError::InvalidRequest)?;
    let session = find_active_session(&state, &session_id).await?;
    assert_identity_can_control_session(&session, &identity)?;
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
    state
        .web_sockets
        .remove_session_user(&session_id, user_id)
        .await;
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
    let emby_protocol = is_emby_protocol_uri(&uri);
    let (requested_id, capabilities) = if emby_protocol {
        let query = emby_capabilities_query(&uri)?;
        let capabilities = ClientCapabilitiesDto {
            playable_media_types: query.playable_media_types,
            supported_commands: query.supported_commands,
            supports_media_control: query.supports_media_control,
            // This property does not exist in Emby's generated DTO. Keep the
            // modern Jellyfin property at its wire default in stored JSON.
            supports_persistent_identifier: false,
            ..ClientCapabilitiesDto::default()
        };
        let mut capabilities =
            serde_json::to_value(capabilities).map_err(|_| ApiError::Internal)?;
        capabilities
            .as_object_mut()
            .expect("ClientCapabilitiesDto serializes as an object")
            .insert("SupportsSync".to_owned(), Value::Bool(query.supports_sync));
        (Some(query.id), capabilities)
    } else {
        let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
        let capabilities = ClientCapabilitiesDto {
            playable_media_types: query.playable_media_types,
            supported_commands: query.supported_commands,
            supports_media_control: query.supports_media_control,
            supports_persistent_identifier: query.supports_persistent_identifier,
            ..ClientCapabilitiesDto::default()
        };
        (
            query.id,
            serde_json::to_value(capabilities).map_err(|_| ApiError::Internal)?,
        )
    };
    let access_token =
        authorized_capabilities_access_token(&state, &identity, &headers, requested_id.as_deref())
            .await?;
    persist_capabilities(&state, &access_token, capabilities).await?;
    crate::websocket::broadcast_sessions(&state).await;
    Ok(if emby_protocol {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    })
}

pub(crate) async fn post_full_capabilities(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<FullCapabilitiesQuery>, QueryRejection>,
    request: Request,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let emby_protocol = is_emby_protocol_uri(&uri);
    let (requested_id, capabilities) = if emby_protocol {
        let requested_id = emby_required_capabilities_id(&uri)?;
        let Json(capabilities) = Json::<EmbyClientCapabilities>::from_request(request, &state)
            .await
            .map_err(|_| ApiError::InvalidRequest)?;
        (Some(requested_id), capabilities.into_stored_value()?)
    } else {
        let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
        let Json(capabilities) = Json::<ClientCapabilitiesDto>::from_request(request, &state)
            .await
            .map_err(|_| ApiError::InvalidRequest)?;
        (
            query.id,
            serde_json::to_value(capabilities).map_err(|_| ApiError::Internal)?,
        )
    };
    let access_token =
        authorized_capabilities_access_token(&state, &identity, &headers, requested_id.as_deref())
            .await?;
    persist_capabilities(&state, &access_token, capabilities).await?;
    crate::websocket::broadcast_sessions(&state).await;
    Ok(if emby_protocol {
        StatusCode::OK
    } else {
        StatusCode::NO_CONTENT
    })
}

#[derive(Debug)]
struct EmbyCapabilitiesQuery {
    id: String,
    playable_media_types: Vec<MediaType>,
    supported_commands: Vec<GeneralCommandType>,
    supports_media_control: bool,
    supports_sync: bool,
}

fn emby_capabilities_query(uri: &axum::http::Uri) -> Result<EmbyCapabilitiesQuery, ApiError> {
    let mut id = None;
    let mut playable_media_types = Vec::new();
    let mut supported_commands = Vec::new();
    let mut supports_media_control = None;
    let mut supports_sync = None;

    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("Id") {
            id = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("PlayableMediaTypes") {
            playable_media_types = emby_capability_list(&value);
        } else if name.eq_ignore_ascii_case("SupportedCommands") {
            supported_commands = emby_capability_list(&value);
        } else if name.eq_ignore_ascii_case("SupportsMediaControl") {
            supports_media_control = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("SupportsSync") {
            supports_sync = Some(value.into_owned());
        }
    }

    let id = id
        .filter(|id| !id.trim().is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    Ok(EmbyCapabilitiesQuery {
        id,
        playable_media_types,
        supported_commands,
        supports_media_control: supports_media_control
            .as_deref()
            .map(emby_bool)
            .transpose()?
            .unwrap_or(false),
        supports_sync: supports_sync
            .as_deref()
            .map(emby_bool)
            .transpose()?
            .unwrap_or(false),
    })
}

fn emby_required_capabilities_id(uri: &axum::http::Uri) -> Result<String, ApiError> {
    let mut id = None;
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("Id") {
            id = Some(value.into_owned());
        }
    }
    id.filter(|id| !id.trim().is_empty())
        .ok_or(ApiError::InvalidRequest)
}

fn emby_capability_list<T>(value: &str) -> Vec<T>
where
    T: std::str::FromStr,
{
    value
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect()
}

fn emby_bool(value: &str) -> Result<bool, ApiError> {
    if value.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if value.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

fn is_emby_protocol_uri(uri: &axum::http::Uri) -> bool {
    uri.path()
        .split('/')
        .find(|segment| !segment.is_empty())
        .is_some_and(|segment| segment.eq_ignore_ascii_case("emby"))
}

/// Emby's generated legacy model owns push, sync, and application identifiers
/// which are intentionally absent from Jellyfin's modern capabilities DTO.
/// The visitor also matches ASP.NET's case-insensitive, last-value-wins
/// top-level JSON binding without teaching the shared DTO about Emby fields.
#[derive(Debug)]
struct EmbyClientCapabilities {
    shared: ClientCapabilitiesDto,
    protocol_fields: serde_json::Map<String, Value>,
}

impl EmbyClientCapabilities {
    fn into_stored_value(self) -> Result<Value, ApiError> {
        let mut value = serde_json::to_value(self.shared).map_err(|_| ApiError::Internal)?;
        value
            .as_object_mut()
            .expect("ClientCapabilitiesDto serializes as an object")
            .extend(self.protocol_fields);
        Ok(value)
    }
}

impl<'de> Deserialize<'de> for EmbyClientCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CapabilitiesVisitor;

        impl<'de> serde::de::Visitor<'de> for CapabilitiesVisitor {
            type Value = EmbyClientCapabilities;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an Emby ClientCapabilities object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut shared = serde_json::Map::new();
                let mut protocol_fields = serde_json::Map::new();
                while let Some(name) = map.next_key::<String>()? {
                    let canonical_shared = if name.eq_ignore_ascii_case("PlayableMediaTypes") {
                        Some("PlayableMediaTypes")
                    } else if name.eq_ignore_ascii_case("SupportedCommands") {
                        Some("SupportedCommands")
                    } else if name.eq_ignore_ascii_case("SupportsMediaControl") {
                        Some("SupportsMediaControl")
                    } else if name.eq_ignore_ascii_case("DeviceProfile") {
                        Some("DeviceProfile")
                    } else if name.eq_ignore_ascii_case("IconUrl") {
                        Some("IconUrl")
                    } else {
                        None
                    };
                    if let Some(canonical) = canonical_shared {
                        shared.insert(canonical.to_owned(), map.next_value()?);
                        continue;
                    }

                    let canonical_protocol = if name.eq_ignore_ascii_case("PushToken") {
                        Some(("PushToken", false))
                    } else if name.eq_ignore_ascii_case("PushTokenType") {
                        Some(("PushTokenType", false))
                    } else if name.eq_ignore_ascii_case("SupportsSync") {
                        Some(("SupportsSync", true))
                    } else if name.eq_ignore_ascii_case("AppId") {
                        Some(("AppId", false))
                    } else {
                        None
                    };
                    if let Some((canonical, _is_boolean)) = canonical_protocol {
                        let value: Value = map.next_value()?;
                        protocol_fields.insert(canonical.to_owned(), value);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                for (name, value) in &protocol_fields {
                    let valid = if name == "SupportsSync" {
                        value.is_boolean()
                    } else {
                        value.is_string() || value.is_null()
                    };
                    if !valid {
                        return Err(A::Error::custom(format!(
                            "invalid Emby ClientCapabilities property {name}"
                        )));
                    }
                }
                let shared =
                    serde_json::from_value(Value::Object(shared)).map_err(A::Error::custom)?;
                Ok(EmbyClientCapabilities {
                    shared,
                    protocol_fields,
                })
            }
        }

        deserializer.deserialize_map(CapabilitiesVisitor)
    }
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

enum SessionController {
    Device(Box<authentication::AuthenticatedSession>),
    ApiKey { session_id: String },
}

impl SessionController {
    fn user_id(&self) -> Uuid {
        match self {
            Self::Device(session) => session.user.id,
            Self::ApiKey { .. } => Uuid::nil(),
        }
    }

    fn session_id(&self) -> String {
        match self {
            Self::Device(session) => {
                jellyfin_session_id(&session.device.app_name, &session.device.device_id)
            }
            Self::ApiKey { session_id } => session_id.clone(),
        }
    }

    fn can_control(&self, target: &device::Model) -> Result<(), ApiError> {
        match self {
            // The official SessionManager treats an API-key request as a
            // privileged controller because its request session has no user.
            Self::ApiKey { .. } => Ok(()),
            Self::Device(session) => assert_can_control_session(target, session),
        }
    }
}

async fn authenticated_session_controller(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<SessionController, ApiError> {
    match authentication::authenticated_identity(state, headers, Some(uri)).await? {
        authentication::AuthenticatedIdentity::Device(session) => {
            Ok(SessionController::Device(session))
        }
        authentication::AuthenticatedIdentity::ApiKey(_) => {
            let client = authentication::authorization_info_from_headers(headers)?;
            Ok(SessionController::ApiKey {
                session_id: jellyfin_session_id(&client.app_name, &client.device_id),
            })
        }
    }
}

fn required_query_value(value: Option<String>) -> Result<String, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(ApiError::InvalidRequest)
}

async fn enqueue_general_command(
    state: &AppState,
    target_session_id: &str,
    controller: &SessionController,
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
    controller: &SessionController,
    message_type: &str,
    payload: T,
) -> Result<(), ApiError>
where
    T: Serialize,
{
    let target = find_active_session(state, target_session_id).await?;
    controller.can_control(&target)?;
    let queued = state
        .session_commands
        .enqueue(NewSessionCommand {
            target_session_id: target_session_id.to_owned(),
            controlling_session_id: Some(controller.session_id()),
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
    user_name: Option<String>,
    user_primary_image_tag: Option<String>,
    server_id: &str,
    has_open_websocket: bool,
    transcoding_info: Option<TranscodingInfo>,
) -> SessionInfoDto {
    let capabilities = ClientCapabilitiesDto::from_stored_value(device.capabilities);
    let play_state: PlayerStateInfo = serde_json::from_value(device.play_state).unwrap_or_default();
    let additional_users: Vec<SessionUserInfo> =
        serde_json::from_value(device.additional_users).unwrap_or_default();
    let now_playing_queue: Vec<serde_json::Value> =
        serde_json::from_value(device.now_playing_queue).unwrap_or_default();
    SessionInfoDto {
        play_state,
        additional_users,
        id: Some(jellyfin_session_id(&device.app_name, &device.device_id)),
        user_id: device.user_id,
        user_name,
        client: Some(device.app_name),
        last_activity_date: device.date_last_activity,
        last_playback_check_in: device.date_last_activity,
        last_paused_date: device.date_last_paused,
        device_name: Some(device.device_name),
        device_type: None,
        now_playing_item: device.now_playing_item,
        device_id: Some(device.device_id),
        application_version: Some(device.app_version),
        transcoding_info,
        is_active: device.is_active,
        supports_media_control: capabilities.supports_media_control && has_open_websocket,
        supports_remote_control: capabilities.supports_media_control && has_open_websocket,
        now_playing_queue,
        has_custom_device_name: false,
        playlist_item_id: device.playlist_item_id,
        server_id: Some(server_id.to_owned()),
        user_primary_image_tag,
        now_viewing_item: device.now_viewing_item,
        playable_media_types: capabilities.playable_media_types.clone(),
        supported_commands: capabilities.supported_commands.clone(),
        capabilities,
    }
}

pub(crate) fn transcoding_info(state: &AppState, device_id: &str) -> Option<TranscodingInfo> {
    state
        .transcode_jobs
        .get_for_device(device_id)
        .map(|job| TranscodingInfo {
            is_video_direct: job.is_video_direct,
            is_audio_direct: job.is_audio_direct,
            transcode_reasons: job.transcode_reasons.names().map(str::to_owned).collect(),
        })
}

fn adapt_emby_transcoding_info(uri: &axum::http::Uri, info: &mut Option<TranscodingInfo>) {
    if !uri
        .path()
        .split('/')
        .nth(1)
        .is_some_and(|segment| segment.eq_ignore_ascii_case("emby"))
    {
        return;
    }
    let Some(info) = info else {
        return;
    };

    // Emby's generated Swift client decodes this as a closed enum. Preserve
    // reasons shared by both protocols, translate the two renamed reasons,
    // and omit Jellyfin-only reasons so one unsupported value cannot reject
    // the complete Sessions response.
    info.transcode_reasons = info
        .transcode_reasons
        .iter()
        .filter_map(|reason| emby_transcode_reason(reason).map(str::to_owned))
        .collect();
}

fn emby_transcode_reason(reason: &str) -> Option<&str> {
    match reason {
        "AudioIsExternal" => Some("ExternalAudioNotSupported"),
        "VideoRangeTypeNotSupported" => Some("VideoRangeNotSupported"),
        "ContainerNotSupported"
        | "VideoCodecNotSupported"
        | "AudioCodecNotSupported"
        | "SubtitleCodecNotSupported"
        | "SecondaryAudioNotSupported"
        | "VideoProfileNotSupported"
        | "VideoLevelNotSupported"
        | "VideoResolutionNotSupported"
        | "VideoBitDepthNotSupported"
        | "VideoFramerateNotSupported"
        | "RefFramesNotSupported"
        | "AnamorphicVideoNotSupported"
        | "InterlacedVideoNotSupported"
        | "AudioChannelsNotSupported"
        | "AudioProfileNotSupported"
        | "AudioSampleRateNotSupported"
        | "AudioBitDepthNotSupported"
        | "ContainerBitrateExceedsLimit"
        | "VideoBitrateNotSupported"
        | "AudioBitrateNotSupported"
        | "UnknownVideoStreamInfo"
        | "UnknownAudioStreamInfo"
        | "DirectPlayError"
        | "VideoRangeNotSupported"
        | "SubtitleContentOptionsEnabled"
        | "ExternalAudioNotSupported"
        | "AudioDelayNotSupported" => Some(reason),
        // VideoCodecTagNotSupported, StreamCountExceedsLimit, and
        // VideoRotationNotSupported have no Emby 4.10 wire representation.
        _ => None,
    }
}

async fn session_user_details(
    state: &AppState,
    devices: &[device::Model],
) -> Result<HashMap<Uuid, (String, Option<String>)>, ApiError> {
    let user_ids = devices
        .iter()
        .map(|device| device.user_id)
        .collect::<Vec<_>>();
    let mut image_tags = user_primary_image_tags(state, &user_ids).await?;
    Ok(state
        .users
        .get_many(&user_ids)
        .await?
        .into_iter()
        .map(|user| {
            let image_tag = image_tags.remove(&user.id);
            (user.id, (user.username, image_tag))
        })
        .collect())
}

async fn persist_capabilities(
    state: &AppState,
    access_token: &str,
    capabilities: Value,
) -> Result<(), ApiError> {
    if state
        .devices
        .update_capabilities_preserving_emby_fields_by_token(access_token, capabilities)
        .await?
        != 1
    {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

fn assert_can_control_session(
    target: &device::Model,
    controller: &authentication::AuthenticatedSession,
) -> Result<(), ApiError> {
    let controller_user_id = controller.user.id;
    if can_control_session(
        target.user_id,
        &target.additional_users,
        controller_user_id,
        authentication::stored_user_policy(&controller.user)?.enable_remote_control_of_other_users,
    ) {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

fn can_control_session(
    target_user_id: Uuid,
    target_additional_users: &serde_json::Value,
    controller_user_id: Uuid,
    can_control_other_users: bool,
) -> bool {
    let additional_users: Vec<SessionUserInfo> =
        serde_json::from_value(target_additional_users.clone()).unwrap_or_default();
    target_user_id.is_nil()
        || target_user_id == controller_user_id
        || additional_users
            .iter()
            .any(|additional| additional.user_id == controller_user_id)
        || can_control_other_users
}

fn assert_identity_can_control_session(
    target: &device::Model,
    identity: &authentication::AuthenticatedIdentity,
) -> Result<(), ApiError> {
    match identity {
        // The official session manager treats a caller without an associated
        // user as a privileged context.
        authentication::AuthenticatedIdentity::ApiKey(_) => Ok(()),
        authentication::AuthenticatedIdentity::Device(controller) => {
            assert_can_control_session(target, controller)
        }
    }
}

fn assert_can_attach_user(
    identity: &authentication::AuthenticatedIdentity,
    user_id: Uuid,
) -> Result<(), ApiError> {
    match identity {
        authentication::AuthenticatedIdentity::ApiKey(_) => Ok(()),
        authentication::AuthenticatedIdentity::Device(controller)
            if controller.user.id == user_id || controller.user.is_administrator =>
        {
            Ok(())
        }
        authentication::AuthenticatedIdentity::Device(_) => Err(ApiError::Forbidden),
    }
}

async fn authorized_capabilities_access_token(
    state: &AppState,
    identity: &authentication::AuthenticatedIdentity,
    headers: &HeaderMap,
    requested_id: Option<&str>,
) -> Result<String, ApiError> {
    let requested_id = requested_id.filter(|value| !value.trim().is_empty());
    if let authentication::AuthenticatedIdentity::Device(session) = identity {
        if requested_id.is_none()
            || requested_id.is_some_and(|id| {
                id == session.device.id.to_string()
                    || id
                        == jellyfin_session_id(&session.device.app_name, &session.device.device_id)
            })
        {
            return Ok(session.access_token.clone());
        }
        let target = find_active_session(state, requested_id.expect("checked above")).await?;
        assert_can_control_session(&target, session)?;
        return Ok(target.access_token);
    }

    // An API key is an unrestricted, user-less controller in the official
    // SessionManager. A supplied id may therefore target any active session.
    // When id is omitted, resolve the request metadata's existing session;
    // PostgreSQL device rows currently require a user and cannot represent a
    // newly created anonymous session.
    let target_session_id = if let Some(requested_id) = requested_id {
        requested_id.to_owned()
    } else {
        let client = authentication::authorization_info_from_headers(headers)?;
        jellyfin_session_id(&client.app_name, &client.device_id)
    };
    let target = find_active_session(state, &target_session_id).await?;
    Ok(target.access_token)
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

#[cfg(test)]
mod tests {
    use super::{
        adapt_emby_transcoding_info, can_access_session_device, can_control_session,
        controlled_user_allows_session, queue_item_ids, transcoding_info,
    };
    use crate::AppState;
    use jellyfin_model::{TranscodeReason, UserPolicy};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn public_and_associated_sessions_follow_official_control_rules() {
        let controller = Uuid::new_v4();
        let foreign_owner = Uuid::new_v4();
        assert!(can_control_session(
            Uuid::nil(),
            &json!([]),
            controller,
            false
        ));
        assert!(can_control_session(
            foreign_owner,
            &json!([{ "UserId": controller.simple().to_string(), "UserName": "Controller" }]),
            controller,
            false
        ));
        assert!(!can_control_session(
            foreign_owner,
            &json!([]),
            controller,
            false
        ));
        assert!(can_control_session(
            foreign_owner,
            &json!([]),
            controller,
            true
        ));
    }

    #[test]
    fn disabling_shared_device_control_only_excludes_public_sessions() {
        assert!(!controlled_user_allows_session(false, Uuid::nil()));
        assert!(controlled_user_allows_session(false, Uuid::new_v4()));
        assert!(controlled_user_allows_session(true, Uuid::nil()));
    }

    #[test]
    fn session_device_access_matches_persistent_identifier_rules() {
        let mut policy = UserPolicy {
            enable_all_devices: false,
            ..UserPolicy::default()
        };
        assert!(can_access_session_device(&policy, "", true));
        assert!(!can_access_session_device(&policy, "restricted", true));
        assert!(can_access_session_device(&policy, "restricted", false));
        policy.enabled_devices = vec!["MiXeD".to_owned()];
        assert!(can_access_session_device(&policy, "mixed", true));
        policy.is_administrator = true;
        assert!(can_access_session_device(&policy, "other", true));
    }

    #[tokio::test]
    async fn session_transcoding_info_exposes_registered_transcode_reasons() {
        let state = AppState::new(
            sea_orm::DatabaseConnection::Disconnected,
            "Session Test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        state
            .transcode_jobs
            .register_for_session("job-1", "device-1", "play-session-1");
        state.transcode_jobs.set_transcode_reasons(
            "job-1",
            TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED
                | TranscodeReason::AUDIO_BITRATE_NOT_SUPPORTED,
        );
        state
            .transcode_jobs
            .set_direct_stream_flags("job-1", true, false);

        let info = transcoding_info(&state, "DEVICE-1").expect("transcoding info");
        assert!(info.is_video_direct);
        assert!(!info.is_audio_direct);
        assert_eq!(
            info.transcode_reasons,
            ["VideoCodecNotSupported", "AudioBitrateNotSupported"]
        );
    }

    #[test]
    fn emby_sessions_translate_or_omit_jellyfin_only_transcode_reasons() {
        let reasons = vec![
            "VideoCodecNotSupported".to_owned(),
            "AudioIsExternal".to_owned(),
            "VideoRangeTypeNotSupported".to_owned(),
            "VideoCodecTagNotSupported".to_owned(),
            "StreamCountExceedsLimit".to_owned(),
            "VideoRotationNotSupported".to_owned(),
        ];
        let mut emby = Some(jellyfin_model::TranscodingInfo {
            transcode_reasons: reasons.clone(),
            ..jellyfin_model::TranscodingInfo::default()
        });
        adapt_emby_transcoding_info(&"/eMbY/Sessions".parse().unwrap(), &mut emby);
        assert_eq!(
            emby.unwrap().transcode_reasons,
            [
                "VideoCodecNotSupported",
                "ExternalAudioNotSupported",
                "VideoRangeNotSupported",
            ]
        );

        let mut jellyfin = Some(jellyfin_model::TranscodingInfo {
            transcode_reasons: reasons.clone(),
            ..jellyfin_model::TranscodingInfo::default()
        });
        adapt_emby_transcoding_info(&"/Sessions".parse().unwrap(), &mut jellyfin);
        assert_eq!(jellyfin.unwrap().transcode_reasons, reasons);

        let mut api = Some(jellyfin_model::TranscodingInfo {
            transcode_reasons: reasons.clone(),
            ..jellyfin_model::TranscodingInfo::default()
        });
        adapt_emby_transcoding_info(&"/api/Sessions".parse().unwrap(), &mut api);
        assert_eq!(api.unwrap().transcode_reasons, reasons);
    }

    #[test]
    fn queue_ids_preserve_order_and_ignore_malformed_entries() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let ids = queue_item_ids(&json!([
            {"Id": first.to_string()},
            {"itemId": "not-a-guid"},
            {"id": second.to_string()}
        ]));
        assert_eq!(ids, vec![first, second]);
    }
}
