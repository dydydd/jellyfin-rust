use std::{collections::HashMap, fmt, net::IpAddr, path::PathBuf, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    body::to_bytes,
    extract::{
        ConnectInfo, OriginalUri, Path, Query, State, rejection::JsonRejection,
        rejection::QueryRejection,
    },
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use jellyfin_data::{NewUserProfileImage, entities::user};
use jellyfin_model::{
    ClientCapabilitiesDto, ForgotPasswordDto, ForgotPasswordPinDto, MimeTypes, PinRedeemResult,
    UserConfiguration, UserDto, UserPolicy,
};
use jellyfin_server_implementations::AuthenticationError;
use serde::{Deserialize, Deserializer, de};
use serde_json::Value;
use uuid::Uuid;

use crate::item_images::parse_image_type;
use crate::{
    ApiError, AppState, authentication, authorization, startup, user_to_dto_with_server_id,
    users_to_dtos_with_server_id,
};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListUsersQuery {
    #[serde(rename = "isHidden", alias = "IsHidden", alias = "ishidden")]
    pub is_hidden: Option<bool>,
    #[serde(rename = "isDisabled", alias = "IsDisabled", alias = "isdisabled")]
    pub is_disabled: Option<bool>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<ListUsersQuery>, QueryRejection>,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let users = state
        .users
        .list_filtered(query.is_hidden, query.is_disabled)
        .await?;
    for user in &users {
        authentication::stored_user_policy(user)?;
    }
    Ok(Json(users_to_dtos_with_server_id(&state, users).await?))
}

pub(crate) async fn list_public(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    authentication::RemoteIp(remote_ip): authentication::RemoteIp,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    let mut users = state.users.list_public().await?;
    if startup::is_completed(&state).await? {
        let device_id = authentication::authorization_info_from_headers(&headers)
            .ok()
            .map(|metadata| metadata.device_id)
            .filter(|device_id| !device_id.trim().is_empty());
        let supports_persistent_identifier = if let Some(device_id) = device_id.as_deref() {
            Some(
                state
                    .devices
                    .latest_by_device_id(device_id)
                    .await?
                    .is_none_or(|device| supports_persistent_identifier(&device.capabilities)),
            )
        } else {
            None
        };
        let is_remote = !state.network_manager.is_in_local_network(remote_ip);
        let mut filtered = Vec::with_capacity(users.len());
        for user in users {
            let policy = authentication::stored_user_policy(&user)?;
            if is_remote && !policy.enable_remote_access {
                continue;
            }
            if let (Some(device_id), Some(supports_persistent_identifier)) =
                (device_id.as_deref(), supports_persistent_identifier)
                && !can_access_device(&policy, device_id, supports_persistent_identifier)
            {
                continue;
            }
            filtered.push(user);
        }
        users = filtered;
    }
    Ok(Json(users_to_dtos_with_server_id(&state, users).await?))
}

fn supports_persistent_identifier(capabilities: &serde_json::Value) -> bool {
    ClientCapabilitiesDto::from_stored_value(capabilities.clone()).supports_persistent_identifier
}

fn can_access_device(
    policy: &UserPolicy,
    device_id: &str,
    supports_persistent_identifier: bool,
) -> bool {
    policy.is_administrator
        || policy.enable_all_devices
        || policy
            .enabled_devices
            .iter()
            .any(|enabled| enabled.eq_ignore_ascii_case(device_id))
        || !supports_persistent_identifier
}

#[derive(Debug, Default)]
pub struct CreateUserByName {
    pub name: Option<String>,
    pub password: Option<String>,
}

impl<'de> Deserialize<'de> for CreateUserByName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = CreateUserByName;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a CreateUserByName object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut request = CreateUserByName::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("Name") {
                        request.name = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("Password") {
                        request.password = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(request)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<CreateUserByName>, JsonRejection>,
) -> Result<Json<UserDto>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let name = request.name.as_deref().ok_or(ApiError::InvalidRequest)?;
    let mut user = state.users.create(name).await?;
    if let Some(password) = request.password.filter(|password| !password.is_empty()) {
        user = hash_and_save_password(&state, user, password).await?;
    }
    let dto = user_to_dto_with_server_id(&state, user).await?;
    crate::websocket::broadcast_user_updated(
        &state,
        &serde_json::to_value(&dto).unwrap_or_default(),
    )
    .await;
    Ok(Json(dto))
}

pub(crate) async fn forgot_password(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
) -> Result<Json<jellyfin_model::ForgotPasswordResult>, ApiError> {
    let remote_ip = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), |info| {
            normalize_ip(info.0.ip())
        });
    let body = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| ApiError::InvalidRequest)?;
    let request: ForgotPasswordDto =
        serde_json::from_slice(&body).map_err(|_| ApiError::InvalidRequest)?;
    let entered_username = request
        .entered_username
        .as_deref()
        .ok_or(ApiError::InvalidRequest)?;
    let is_in_network = state.network_manager.is_in_local_network(remote_ip);
    Ok(Json(
        state
            .users
            .start_forgot_password_process(entered_username, is_in_network)
            .await?,
    ))
}

pub(crate) async fn forgot_password_pin(
    State(state): State<Arc<AppState>>,
    request: Result<Json<ForgotPasswordPinDto>, JsonRejection>,
) -> Result<Json<PinRedeemResult>, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let pin = request.pin.as_deref().ok_or(ApiError::InvalidRequest)?;
    let password_hash = state.authentication.password_hash(pin);
    let users_reset = state
        .users
        .redeem_password_reset_pin(pin, password_hash)
        .await?;
    Ok(Json(PinRedeemResult {
        success: true,
        users_reset,
    }))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, ApiError> {
    authorization::require_ignore_parental_control(&state, &headers, &uri).await?;
    let user = state.users.get(id).await?;
    authentication::stored_user_policy(&user)?;
    Ok(Json(user_to_dto_with_server_id(&state, user).await?))
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateUserQuery {
    #[serde(rename = "userId", alias = "UserId", alias = "userid")]
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetUserImageQuery {
    #[serde(default, rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "tag", alias = "Tag")]
    tag: Option<String>,
    #[serde(default, rename = "format", alias = "Format")]
    format: Option<String>,
}

pub(crate) async fn get_user_image(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<GetUserImageQuery>,
) -> Result<Response, ApiError> {
    let authenticated_user_id =
        authentication::optional_authenticated_user_id(&state, &headers, &uri).await?;
    let user_id = query
        .user_id
        .or(authenticated_user_id)
        .filter(|user_id| !user_id.is_nil())
        .ok_or(ApiError::InvalidRequest)?;
    get_user_image_for(
        &state,
        &headers,
        user_id,
        query.tag.as_deref(),
        query.format.as_deref(),
    )
    .await
}

pub(crate) async fn get_user_image_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, image_type)): Path<(Uuid, String)>,
    Query(query): Query<GetUserImageQuery>,
) -> Result<Response, ApiError> {
    parse_image_type(&image_type)?;
    get_user_image_for(
        &state,
        &headers,
        user_id,
        query.tag.as_deref(),
        query.format.as_deref(),
    )
    .await
}

pub(crate) async fn get_user_image_index_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, image_type, _index)): Path<(Uuid, String, i32)>,
    Query(query): Query<GetUserImageQuery>,
) -> Result<Response, ApiError> {
    parse_image_type(&image_type)?;
    get_user_image_for(
        &state,
        &headers,
        user_id,
        query.tag.as_deref(),
        query.format.as_deref(),
    )
    .await
}

async fn get_user_image_for(
    state: &AppState,
    headers: &HeaderMap,
    user_id: Uuid,
    tag: Option<&str>,
    format: Option<&str>,
) -> Result<Response, ApiError> {
    if user_id.is_nil() {
        return Err(ApiError::InvalidRequest);
    }
    let image = state
        .users
        .profile_image(user_id)
        .await
        .map_err(|_| ApiError::Internal)?
        .ok_or(jellyfin_controller::UserError::NotFound)?;
    crate::item_images::render_simple_image(
        state,
        headers,
        PathBuf::from(image.path),
        image.last_modified,
        tag,
        format,
    )
    .await
}

pub(crate) async fn post_user_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    let target_id =
        management_target_id(&identity, query.user_id.filter(|user_id| !user_id.is_nil()));
    post_user_image_for(&state, &headers, &identity, target_id, request).await
}

pub(crate) async fn post_user_image_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, image_type)): Path<(Uuid, String)>,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    parse_image_type(&image_type)?;
    post_user_image_for(&state, &headers, &identity, target_id, request).await
}

pub(crate) async fn post_user_image_index_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, image_type, _index)): Path<(Uuid, String, u32)>,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    parse_image_type(&image_type)?;
    post_user_image_for(&state, &headers, &identity, target_id, request).await
}

async fn post_user_image_for(
    state: &AppState,
    headers: &HeaderMap,
    identity: &authentication::AuthenticatedIdentity,
    target_id: Uuid,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_identity_can_update_user(identity, &target)?;
    let extension = MimeTypes::try_get_image_extension(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
    )
    .ok_or(ApiError::InvalidRequest)?;
    let encoded = to_bytes(request.into_body(), 16 * 1024 * 1024)
        .await
        .map_err(|_| ApiError::PayloadTooLarge)?;
    let image = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::InvalidRequest)?;
    if image.is_empty() {
        return Err(ApiError::InvalidRequest);
    }

    let user_directory = profile_image_directory(state, target.id);
    tokio::fs::create_dir_all(&user_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let path = user_directory.join(format!("profile{extension}"));
    let temporary_path = user_directory.join(format!("profile-{}.tmp", Uuid::new_v4().simple()));
    tokio::fs::write(&temporary_path, image)
        .await
        .map_err(|_| ApiError::Internal)?;
    tokio::fs::rename(&temporary_path, &path)
        .await
        .map_err(|_| ApiError::Internal)?;

    let previous = state
        .users
        .profile_image(target.id)
        .await
        .map_err(|_| ApiError::Internal)?;
    state
        .users
        .set_profile_image(NewUserProfileImage {
            user_id: target.id,
            path: path_string(&path),
            last_modified: Utc::now(),
        })
        .await
        .map_err(|_| ApiError::Internal)?;
    if let Some(previous) = previous
        && previous.path != path_string(&path)
    {
        let _ = tokio::fs::remove_file(previous.path).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_user_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    let target_id =
        management_target_id(&identity, query.user_id.filter(|user_id| !user_id.is_nil()));
    delete_user_image_for(&state, &identity, target_id).await
}

pub(crate) async fn delete_user_image_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, image_type)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    parse_image_type(&image_type)?;
    delete_user_image_for(&state, &identity, target_id).await
}

pub(crate) async fn delete_user_image_index_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, image_type, _index)): Path<(Uuid, String, u32)>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    parse_image_type(&image_type)?;
    delete_user_image_for(&state, &identity, target_id).await
}

async fn delete_user_image_for(
    state: &AppState,
    identity: &authentication::AuthenticatedIdentity,
    target_id: Uuid,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_identity_can_update_user(identity, &target)?;
    let removed = state
        .users
        .clear_profile_image(target.id)
        .await
        .map_err(|_| ApiError::Internal)?;
    if let Some(image) = removed {
        let _ = tokio::fs::remove_file(image.path).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn profile_image_directory(state: &AppState, user_id: Uuid) -> PathBuf {
    state
        .program_data_directory
        .join("users")
        .join(user_id.simple().to_string())
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UserDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let target_id = management_target_id(&identity, query.user_id);
    update_with_id(&state, &identity, target_id, request).await
}

pub(crate) async fn update_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UserDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    update_with_id(&state, &identity, target_id, request).await
}

async fn update_with_id(
    state: &AppState,
    identity: &authentication::AuthenticatedIdentity,
    target_id: Uuid,
    request: Result<Json<UserDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_identity_can_update_user(identity, &target)?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let name = request.name.as_deref().ok_or(ApiError::InvalidRequest)?;
    if target.username != name {
        state.users.rename(target_id, name).await?;
    }
    state
        .users
        .update_configuration(target_id, &request.configuration)
        .await?;
    let dto = user_to_dto_with_server_id(state, state.users.get(target_id).await?).await?;
    crate::websocket::broadcast_user_updated(
        state,
        &serde_json::to_value(&dto).unwrap_or_default(),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UserConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let target_id = management_target_id(&identity, query.user_id);
    update_configuration_with_id(&state, &identity, target_id, request).await
}

pub(crate) async fn update_configuration_legacy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UserConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    update_configuration_with_id(&state, &identity, target_id, request).await
}

pub(crate) async fn update_configuration_partial(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let target = state.users.get(target_id).await?;
    assert_identity_can_update_user(&identity, &target)?;
    let patch: Value = serde_json::from_slice(&body).map_err(|_| ApiError::InvalidRequest)?;
    let Some(patch) = patch.as_object() else {
        return Err(ApiError::InvalidRequest);
    };
    let mut merged = serde_json::to_value(
        UserConfiguration::deserialize(&target.preferences).unwrap_or_default(),
    )
    .map_err(|_| ApiError::Internal)?;
    let object = merged.as_object_mut().ok_or(ApiError::Internal)?;
    for (key, value) in patch {
        let destination = object
            .keys()
            .find(|existing| existing.eq_ignore_ascii_case(key))
            .cloned()
            .unwrap_or_else(|| key.clone());
        object.insert(destination, value.clone());
    }
    let configuration: UserConfiguration =
        serde_json::from_value(merged).map_err(|_| ApiError::InvalidRequest)?;
    state
        .users
        .update_configuration(target_id, &configuration)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_configuration_with_id(
    state: &AppState,
    identity: &authentication::AuthenticatedIdentity,
    target_id: Uuid,
    request: Result<Json<UserConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_identity_can_update_user(identity, &target)?;
    let Json(configuration) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .users
        .update_configuration(target.id, &configuration)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default)]
pub struct UpdateUserPassword {
    pub current_pw: Option<String>,
    pub new_pw: Option<String>,
    pub reset_password: bool,
}

impl<'de> Deserialize<'de> for UpdateUserPassword {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UpdateUserPassword;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an UpdateUserPassword object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut request = UpdateUserPassword::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("CurrentPw") {
                        request.current_pw = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("NewPw") {
                        request.new_pw = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("ResetPassword") {
                        request.reset_password = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(request)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

pub(crate) async fn update_password(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    update_password_with_id(&state, &identity, target_id, true, request).await
}

pub(crate) async fn update_password_query(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let has_explicit_user_id = query.user_id.is_some();
    let target_id = management_target_id(&identity, query.user_id);
    update_password_with_id(&state, &identity, target_id, has_explicit_user_id, request).await
}

async fn update_password_with_id(
    state: &AppState,
    identity: &authentication::AuthenticatedIdentity,
    target_id: Uuid,
    has_explicit_user_id: bool,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let mut target = state.users.get(target_id).await?;
    assert_identity_can_update_user(identity, &target)?;
    if request.reset_password {
        // Jellyfin only revokes tokens after a password change. Its reset
        // branch clears the password while leaving existing sessions active.
        hash_and_save_password(state, target, String::new()).await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    // The official controller lets an administrator change their own
    // password without CurrentPw only when the modern route omits userId.
    if let authentication::AuthenticatedIdentity::Device(session) = identity
        && session.user.id == target_id
        && (!session.user.is_administrator || has_explicit_user_id)
    {
        target =
            verify_current_password(state, target, request.current_pw.unwrap_or_default()).await?;
    }
    hash_and_save_password(state, target, request.new_pw.unwrap_or_default()).await?;
    state
        .devices
        .revoke_user_tokens(target_id, Some(identity.access_token()))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    state.users.delete(target_id).await?;
    crate::websocket::broadcast_user_deleted(&state, target_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_policy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<CaseInsensitiveUserPolicy>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    identity.require_administrator()?;
    let current_token = identity.access_token().to_owned();
    let Json(CaseInsensitiveUserPolicy(policy)) = request.map_err(|_| ApiError::InvalidRequest)?;
    let (_, became_disabled) = state.users.update_policy(target_id, &policy).await?;
    if became_disabled {
        state
            .devices
            .revoke_user_tokens(target_id, Some(&current_token))
            .await?;
    }
    let dto = user_to_dto_with_server_id(&state, state.users.get(target_id).await?).await?;
    crate::websocket::broadcast_user_updated(
        &state,
        &serde_json::to_value(&dto).unwrap_or_default(),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) struct CaseInsensitiveUserPolicy(UserPolicy);

impl<'de> Deserialize<'de> for CaseInsensitiveUserPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PolicyVisitor;

        impl<'de> de::Visitor<'de> for PolicyVisitor {
            type Value = CaseInsensitiveUserPolicy;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a user policy object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut normalized = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value = map.next_value::<Value>()?;
                    if let Some(field) = canonical_user_policy_field(&key) {
                        // ASP.NET's JSON binder matches property names without
                        // regard to case and the last duplicate value wins.
                        normalized.insert(field.to_owned(), value);
                    }
                }
                serde_json::from_value(Value::Object(normalized))
                    .map(CaseInsensitiveUserPolicy)
                    .map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_map(PolicyVisitor)
    }
}

fn canonical_user_policy_field(name: &str) -> Option<&'static str> {
    const FIELDS: &[&str] = &[
        "IsAdministrator",
        "IsHidden",
        "EnableCollectionManagement",
        "EnableSubtitleManagement",
        "EnableLyricManagement",
        "IsDisabled",
        "MaxParentalRating",
        "MaxParentalSubRating",
        "BlockedTags",
        "AllowedTags",
        "EnableUserPreferenceAccess",
        "AccessSchedules",
        "BlockUnratedItems",
        "EnableRemoteControlOfOtherUsers",
        "EnableSharedDeviceControl",
        "EnableRemoteAccess",
        "EnableLiveTvManagement",
        "EnableLiveTvAccess",
        "EnableMediaPlayback",
        "EnableAudioPlaybackTranscoding",
        "EnableVideoPlaybackTranscoding",
        "EnablePlaybackRemuxing",
        "ForceRemoteSourceTranscoding",
        "EnableContentDeletion",
        "EnableContentDeletionFromFolders",
        "EnableContentDownloading",
        "EnableSyncTranscoding",
        "EnableMediaConversion",
        "EnabledDevices",
        "EnableAllDevices",
        "EnabledChannels",
        "EnableAllChannels",
        "EnabledFolders",
        "EnableAllFolders",
        "InvalidLoginAttemptCount",
        "LoginAttemptsBeforeLockout",
        "MaxActiveSessions",
        "EnablePublicSharing",
        "BlockedMediaFolders",
        "BlockedChannels",
        "RemoteClientBitrateLimit",
        "AuthenticationProviderId",
        "PasswordResetProviderId",
        "SyncPlayAccess",
    ];
    FIELDS
        .iter()
        .find(|field| field.eq_ignore_ascii_case(name))
        .copied()
}

fn management_target_id(
    identity: &authentication::AuthenticatedIdentity,
    requested: Option<Uuid>,
) -> Uuid {
    requested.unwrap_or_else(|| match identity {
        authentication::AuthenticatedIdentity::Device(session) => session.user.id,
        authentication::AuthenticatedIdentity::ApiKey(_) => Uuid::nil(),
    })
}

fn assert_identity_can_update_user(
    identity: &authentication::AuthenticatedIdentity,
    target: &user::Model,
) -> Result<(), ApiError> {
    // CustomAuthenticationHandler gives API keys the Administrator role, and
    // RequestHelpers.AssertCanUpdateUser therefore permits their explicit target.
    // Keep lookup before this check, including an API key's omitted/nil 404.
    match identity {
        authentication::AuthenticatedIdentity::Device(session) => {
            assert_can_update_user(&session.user, target)
        }
        authentication::AuthenticatedIdentity::ApiKey(_) => Ok(()),
    }
}

impl AppState {
    /// Resolves and authorizes an Emby password target before parsing the
    /// generated protocol body. Emby's DTO has no current-password field, so
    /// an authorized user may change their own password without the Jellyfin
    /// protocol's separate `CurrentPw` requirement.
    pub async fn emby_password_update_target(
        &self,
        headers: &HeaderMap,
        uri: &axum::http::Uri,
        target_id: Uuid,
    ) -> Result<String, Response> {
        let identity = authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map_err(IntoResponse::into_response)?;
        let target = self
            .users
            .get(target_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        assert_identity_can_update_user(&identity, &target).map_err(IntoResponse::into_response)?;
        Ok(identity.access_token().to_owned())
    }

    /// Persists an Emby password mutation while retaining Jellyfin's token
    /// revocation distinction between password changes and resets.
    pub async fn persist_emby_password(
        &self,
        target_id: Uuid,
        new_password: String,
        reset_password: bool,
        current_token: &str,
    ) -> Result<StatusCode, Response> {
        let target = self
            .users
            .get(target_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        hash_and_save_password(
            self,
            target,
            if reset_password {
                String::new()
            } else {
                new_password
            },
        )
        .await
        .map_err(IntoResponse::into_response)?;
        if !reset_password {
            self.devices
                .revoke_user_tokens(target_id, Some(current_token))
                .await
                .map_err(ApiError::from)
                .map_err(IntoResponse::into_response)?;
        }
        Ok(StatusCode::OK)
    }

    /// Resolves and authorizes an Emby configuration target before the
    /// protocol adapter parses its request body, preserving official lookup
    /// and authorization precedence.
    pub async fn emby_configuration_update_target(
        &self,
        headers: &HeaderMap,
        uri: &axum::http::Uri,
        target_id: Uuid,
    ) -> Result<UserConfiguration, Response> {
        let identity = authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map_err(IntoResponse::into_response)?;
        let target = self
            .users
            .get(target_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        assert_identity_can_update_user(&identity, &target).map_err(IntoResponse::into_response)?;
        let mut configuration =
            UserConfiguration::deserialize(&target.preferences).unwrap_or_default();
        configuration.enable_local_password = target.enable_local_password;
        Ok(configuration)
    }

    /// Resolves an Emby policy target under the shared elevated boundary
    /// before the adapter parses its protocol-local body.
    pub async fn emby_policy_update_target(
        &self,
        headers: &HeaderMap,
        uri: &axum::http::Uri,
        target_id: Uuid,
    ) -> Result<(UserPolicy, String), Response> {
        let identity = authentication::authenticated_identity(self, headers, Some(uri))
            .await
            .map_err(IntoResponse::into_response)?;
        identity
            .require_administrator()
            .map_err(IntoResponse::into_response)?;
        let target = self
            .users
            .get(target_id)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        let policy =
            authentication::stored_user_policy(&target).map_err(IntoResponse::into_response)?;
        Ok((policy, identity.access_token().to_owned()))
    }

    /// Persists a validated Emby configuration and its shared Jellyfin view.
    pub async fn persist_emby_configuration(
        &self,
        target_id: Uuid,
        configuration: &UserConfiguration,
        emby_configuration: serde_json::Value,
    ) -> Result<StatusCode, Response> {
        self.users
            .update_emby_configuration(target_id, configuration, emby_configuration)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        Ok(StatusCode::OK)
    }

    /// Persists a validated Emby policy, revokes sessions when disabling the
    /// target, and emits the same user-updated event as Jellyfin's mutation.
    pub async fn persist_emby_policy(
        &self,
        target_id: Uuid,
        policy: &UserPolicy,
        emby_policy: serde_json::Value,
        current_token: &str,
    ) -> Result<StatusCode, Response> {
        let (_, became_disabled) = self
            .users
            .update_emby_policy(target_id, policy, emby_policy)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        if became_disabled {
            self.devices
                .revoke_user_tokens(target_id, Some(current_token))
                .await
                .map_err(ApiError::from)
                .map_err(IntoResponse::into_response)?;
        }
        let dto = user_to_dto_with_server_id(
            self,
            self.users
                .get(target_id)
                .await
                .map_err(ApiError::from)
                .map_err(IntoResponse::into_response)?,
        )
        .await
        .map_err(IntoResponse::into_response)?;
        crate::websocket::broadcast_user_updated(
            self,
            &serde_json::to_value(&dto).unwrap_or_default(),
        )
        .await;
        Ok(StatusCode::OK)
    }

    /// Batch-loads the PostgreSQL documents used to adapt Jellyfin UserDto
    /// values into the Emby wire contract without one query per user.
    pub async fn emby_user_contract_storage(
        &self,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, (Value, Value, bool)>, Response> {
        let users = self
            .users
            .get_many(ids)
            .await
            .map_err(ApiError::from)
            .map_err(IntoResponse::into_response)?;
        Ok(users
            .into_iter()
            .map(|user| {
                (
                    user.id,
                    (user.preferences, user.policy, user.enable_local_password),
                )
            })
            .collect())
    }
}

/// Matches Jellyfin's `RequestHelpers.AssertCanUpdateUser` semantics for
/// profile, configuration, password and image mutations. Administrators may
/// update any user; non-administrators may only update themselves when their
/// persisted policy enables user preference access.
fn assert_can_update_user(
    authenticated_user: &user::Model,
    target_user: &user::Model,
) -> Result<(), ApiError> {
    if authenticated_user.is_administrator {
        return Ok(());
    }
    if authenticated_user.id != target_user.id {
        return Err(ApiError::Forbidden);
    }
    let policy = authentication::stored_user_policy(target_user)?;
    if policy.enable_user_preference_access {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn verify_current_password(
    state: &AppState,
    mut user: user::Model,
    current_password: String,
) -> Result<user::Model, ApiError> {
    let authentication = state.authentication;
    let (result, user) = tokio::task::spawn_blocking(move || {
        let username = std::mem::take(&mut user.username);
        let result = authentication.authenticate(&username, &current_password, Some(&mut user));
        user.username = username;
        (result, user)
    })
    .await
    .map_err(|_| ApiError::Internal)?;
    match result {
        Ok(_) => Ok(user),
        Err(AuthenticationError::InvalidCredentials) => Err(ApiError::Forbidden),
        Err(error) => Err(error.into()),
    }
}

async fn hash_and_save_password(
    state: &AppState,
    mut user: user::Model,
    new_password: String,
) -> Result<user::Model, ApiError> {
    let authentication = state.authentication;
    let user = tokio::task::spawn_blocking(move || {
        authentication.change_password(&mut user, &new_password);
        user
    })
    .await
    .map_err(|_| ApiError::Internal)?;
    Ok(state
        .users
        .set_password_hash(user.id, user.password_hash)
        .await?)
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address @ IpAddr::V4(_) => address,
    }
}

#[cfg(test)]
mod user_image_query_tests {
    use axum_extra::extract::Query;
    use uuid::Uuid;

    use super::GetUserImageQuery;

    #[test]
    fn user_image_query_binds_lowercase_userid() {
        let user_id = Uuid::new_v4();
        let uri = format!("http://localhost/?userid={user_id}")
            .parse()
            .unwrap();
        let query = Query::<GetUserImageQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.user_id, Some(user_id));
    }
}

#[cfg(test)]
mod user_policy_request_tests {
    use super::CaseInsensitiveUserPolicy;

    #[test]
    fn user_policy_properties_bind_case_insensitively_and_last_value_wins() {
        let CaseInsensitiveUserPolicy(policy) = serde_json::from_str(
            r#"{
                "isadministrator": false,
                "ENABLECONTENTDOWNLOADING": false,
                "enableContentDownloading": true,
                "aUtHeNtIcAtIoNpRoViDeRiD": "auth-provider",
                "PASSWORDRESETPROVIDERID": "reset-provider"
            }"#,
        )
        .expect("case-insensitive user policy");

        assert!(!policy.is_administrator);
        assert!(policy.enable_content_downloading);
        assert_eq!(
            policy.authentication_provider_id.as_deref(),
            Some("auth-provider")
        );
        assert_eq!(
            policy.password_reset_provider_id.as_deref(),
            Some("reset-provider")
        );
    }
}

#[cfg(test)]
mod user_password_request_tests {
    use super::UpdateUserPassword;

    #[test]
    fn password_properties_bind_case_insensitively_and_last_value_wins() {
        let request: UpdateUserPassword = serde_json::from_str(
            r#"{
                "CurrentPw":"old-first",
                "cUrReNtPw":"old-last",
                "NewPw":"new-first",
                "nEwPw":"new-last",
                "ResetPassword":true,
                "rEsEtPaSsWoRd":false,
                "Ignored":"value"
            }"#,
        )
        .expect("case-insensitive password update");

        assert_eq!(request.current_pw.as_deref(), Some("old-last"));
        assert_eq!(request.new_pw.as_deref(), Some("new-last"));
        assert!(!request.reset_password);
    }
}

#[cfg(test)]
mod create_user_request_tests {
    use super::CreateUserByName;

    #[test]
    fn create_properties_bind_case_insensitively_and_last_value_wins() {
        let request: CreateUserByName = serde_json::from_str(
            r#"{
                "Name":"first",
                "nAmE":"last",
                "Password":"old",
                "pAsSwOrD":"new",
                "Ignored":"value"
            }"#,
        )
        .expect("case-insensitive user creation");

        assert_eq!(request.name.as_deref(), Some("last"));
        assert_eq!(request.password.as_deref(), Some("new"));
    }
}
