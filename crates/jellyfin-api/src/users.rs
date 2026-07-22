use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::entities::user;
use jellyfin_model::{UserDto, UserPolicy};
use jellyfin_server_implementations::AuthenticationError;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization, user_to_dto};

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let users = state.users.list().await?;
    for user in &users {
        authentication::stored_user_policy(user)?;
    }
    Ok(Json(users.into_iter().map(user_to_dto).collect()))
}

pub(crate) async fn list_public(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<UserDto>>, ApiError> {
    Ok(Json(
        state
            .users
            .list_public()
            .await?
            .into_iter()
            .map(user_to_dto)
            .collect(),
    ))
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
    Ok(Json(user_to_dto(user)))
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
    Ok(Json(user_to_dto(user)))
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateUserQuery {
    #[serde(rename = "userId")]
    pub user_id: Option<Uuid>,
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
    if !authenticated_user.is_administrator && authenticated_user.id != target_id {
        return Err(ApiError::Forbidden);
    }
    state.users.get(target_id).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let name = request.name.as_deref().ok_or(ApiError::InvalidRequest)?;
    state.users.rename(target_id, name).await?;
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
    update_password_with_id(&state, authenticated, target_id, request).await
}

pub(crate) async fn update_password_query(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpdateUserQuery>,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_id = query.user_id.unwrap_or(authenticated.user.id);
    update_password_with_id(&state, authenticated, target_id, request).await
}

async fn update_password_with_id(
    state: &AppState,
    authenticated: authentication::AuthenticatedSession,
    target_id: Uuid,
    request: Result<Json<UpdateUserPassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let target = state.users.get(target_id).await?;
    if !authenticated.user.is_administrator && authenticated.user.id != target_id {
        return Err(ApiError::Forbidden);
    }
    if authenticated.user.id == target_id && !request.reset_password {
        verify_current_password(
            state,
            target.clone(),
            request.current_pw.unwrap_or_default(),
        )
        .await?;
    }

    let new_password = if request.reset_password {
        String::new()
    } else {
        request.new_pw.unwrap_or_default()
    };
    hash_and_save_password(state, target, new_password).await?;
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

async fn verify_current_password(
    state: &AppState,
    mut user: user::Model,
    current_password: String,
) -> Result<(), ApiError> {
    let authentication = state.authentication;
    let username = user.username.clone();
    let result = tokio::task::spawn_blocking(move || {
        authentication.authenticate(&username, &current_password, Some(&mut user))
    })
    .await
    .map_err(|_| ApiError::Internal)?;
    match result {
        Ok(_) => Ok(()),
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
