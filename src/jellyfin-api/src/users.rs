use std::{net::IpAddr, path::PathBuf, sync::Arc};

use axum::{
    Json,
    body::to_bytes,
    extract::{
        ConnectInfo, OriginalUri, Path, Query, State, rejection::JsonRejection,
        rejection::QueryRejection,
    },
    http::{HeaderMap, Request, StatusCode, header},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use jellyfin_data::{NewUserProfileImage, entities::user};
use jellyfin_model::{
    ClientCapabilitiesDto, ForgotPasswordDto, ForgotPasswordPinDto, MimeTypes, PinRedeemResult,
    UserConfiguration, UserDto, UserPolicy,
};
use jellyfin_server_implementations::AuthenticationError;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication, authorization, startup, user_to_dto_with_server_id,
    users_to_dtos_with_server_id,
};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListUsersQuery {
    #[serde(rename = "isHidden", alias = "IsHidden")]
    pub is_hidden: Option<bool>,
    #[serde(rename = "isDisabled", alias = "IsDisabled")]
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

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CreateUserByName {
    pub name: Option<String>,
    pub password: Option<String>,
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Result<Json<CreateUserByName>, JsonRejection>,
) -> Result<Json<UserDto>, ApiError> {
    require_administrator(&state, &headers).await?;
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
    #[serde(rename = "userId", alias = "UserId")]
    pub user_id: Option<Uuid>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetUserImageQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
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
    Path((user_id, _image_type)): Path<(Uuid, String)>,
    Query(query): Query<GetUserImageQuery>,
) -> Result<Response, ApiError> {
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
    Path((user_id, _image_type, _index)): Path<(Uuid, String, i32)>,
    Query(query): Query<GetUserImageQuery>,
) -> Result<Response, ApiError> {
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
        90,
    )
    .await
}

pub(crate) async fn post_user_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_id = query.user_id.unwrap_or(authenticated.user.id);
    post_user_image_for(&state, &headers, authenticated.user, target_id, request).await
}

pub(crate) async fn post_user_image_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, _image_type)): Path<(Uuid, String)>,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    post_user_image_for(&state, &headers, authenticated.user, target_id, request).await
}

pub(crate) async fn post_user_image_index_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, _image_type, _index)): Path<(Uuid, String, u32)>,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    post_user_image_for(&state, &headers, authenticated.user, target_id, request).await
}

async fn post_user_image_for(
    state: &AppState,
    headers: &HeaderMap,
    authenticated_user: user::Model,
    target_id: Uuid,
    request: Request<axum::body::Body>,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_can_update_user(&authenticated_user, &target)?;
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
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_id = query.user_id.unwrap_or(authenticated.user.id);
    delete_user_image_for(&state, authenticated.user, target_id).await
}

pub(crate) async fn delete_user_image_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, _image_type)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    delete_user_image_for(&state, authenticated.user, target_id).await
}

pub(crate) async fn delete_user_image_index_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((target_id, _image_type, _index)): Path<(Uuid, String, u32)>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    delete_user_image_for(&state, authenticated.user, target_id).await
}

async fn delete_user_image_for(
    state: &AppState,
    authenticated_user: user::Model,
    target_id: Uuid,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_can_update_user(&authenticated_user, &target)?;
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
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UserDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_id = query.user_id.unwrap_or(authenticated.user.id);
    update_with_id(&state, authenticated.user, target_id, request).await
}

pub(crate) async fn update_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UserDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    update_with_id(&state, authenticated.user, target_id, request).await
}

async fn update_with_id(
    state: &AppState,
    authenticated_user: user::Model,
    target_id: Uuid,
    request: Result<Json<UserDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_can_update_user(&authenticated_user, &target)?;
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
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UserConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_id = query.user_id.unwrap_or(authenticated.user.id);
    update_configuration_with_id(&state, authenticated.user, target_id, request).await
}

pub(crate) async fn update_configuration_legacy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UserConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    update_configuration_with_id(&state, authenticated.user, target_id, request).await
}

async fn update_configuration_with_id(
    state: &AppState,
    authenticated_user: user::Model,
    target_id: Uuid,
    request: Result<Json<UserConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let target = state.users.get(target_id).await?;
    assert_can_update_user(&authenticated_user, &target)?;
    let Json(configuration) = request.map_err(|_| ApiError::InvalidRequest)?;
    state
        .users
        .update_configuration(target.id, &configuration)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UpdateUserPassword {
    pub current_pw: Option<String>,
    pub new_pw: Option<String>,
    pub reset_password: bool,
}

pub(crate) async fn update_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    update_password_with_id(&state, authenticated, target_id, true, request).await
}

pub(crate) async fn update_password_query(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let has_explicit_user_id = query.user_id.is_some();
    let target_id = query.user_id.unwrap_or(authenticated.user.id);
    update_password_with_id(
        &state,
        authenticated,
        target_id,
        has_explicit_user_id,
        request,
    )
    .await
}

async fn update_password_with_id(
    state: &AppState,
    authenticated: authentication::AuthenticatedSession,
    target_id: Uuid,
    has_explicit_user_id: bool,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let mut target = state.users.get(target_id).await?;
    assert_can_update_user(&authenticated.user, &target)?;
    if request.reset_password {
        // Jellyfin only revokes tokens after a password change. Its reset
        // branch clears the password while leaving existing sessions active.
        hash_and_save_password(state, target, String::new()).await?;
        return Ok(StatusCode::NO_CONTENT);
    }

    // The official controller lets an administrator change their own
    // password without CurrentPw only when the modern route omits userId.
    if authenticated.user.id == target_id
        && (!authenticated.user.is_administrator || has_explicit_user_id)
    {
        target =
            verify_current_password(state, target, request.current_pw.unwrap_or_default()).await?;
    }
    hash_and_save_password(state, target, request.new_pw.unwrap_or_default()).await?;
    state
        .devices
        .revoke_user_tokens(target_id, Some(&authenticated.access_token))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    require_administrator(&state, &headers).await?;
    state.users.delete(target_id).await?;
    crate::websocket::broadcast_user_deleted(&state, target_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_policy(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    request: Result<Json<UserPolicy>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    identity.require_administrator()?;
    let current_token = identity.access_token().to_owned();
    let Json(policy) = request.map_err(|_| ApiError::InvalidRequest)?;
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

async fn require_administrator(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<user::Model, ApiError> {
    let authenticated = authentication::authenticated_session(state, headers).await?;
    if authenticated.user.is_administrator {
        Ok(authenticated.user)
    } else {
        Err(ApiError::Forbidden)
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
