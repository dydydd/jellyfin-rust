use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Query, State, rejection::QueryRejection},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use jellyfin_server_implementations::QuickConnectResult;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ConnectQuery {
    secret: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AuthorizeQuery {
    code: Option<String>,
    user_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct QuickConnectResultDto {
    authenticated: bool,
    secret: String,
    code: String,
    device_id: String,
    device_name: String,
    app_name: String,
    app_version: String,
    date_added: DateTime<Utc>,
}

impl From<QuickConnectResult> for QuickConnectResultDto {
    fn from(result: QuickConnectResult) -> Self {
        Self {
            authenticated: result.authenticated,
            secret: result.secret,
            code: result.code,
            device_id: result.device_id,
            device_name: result.device_name,
            app_name: result.app_name,
            app_version: result.app_version,
            date_added: result.date_added,
        }
    }
}

pub(crate) async fn enabled(State(state): State<Arc<AppState>>) -> Json<bool> {
    Json(state.quick_connect.is_enabled())
}

pub(crate) async fn initiate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<QuickConnectResultDto>, ApiError> {
    let authorization = authentication::authorization_info_from_headers(&headers)?;
    Ok(Json(
        state
            .quick_connect
            .try_connect(&authorization)
            .await?
            .into(),
    ))
}

pub(crate) async fn connect(
    State(state): State<Arc<AppState>>,
    query: Result<Query<ConnectQuery>, QueryRejection>,
) -> Result<Json<QuickConnectResultDto>, ApiError> {
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let secret = required_query_value(query.secret)?;
    Ok(Json(
        state
            .quick_connect
            .check_request_status(&secret)
            .await?
            .into(),
    ))
}

pub(crate) async fn authorize(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<AuthorizeQuery>, QueryRejection>,
) -> Result<Json<bool>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let code = required_query_value(query.code)?;
    let user_id = identity.target_user_id(query.user_id)?;
    if user_id.is_nil() {
        return Err(ApiError::InvalidRequest);
    }
    state.users.get(user_id).await?;
    Ok(Json(
        state
            .quick_connect
            .authorize_request(user_id, &code)
            .await?,
    ))
}

fn required_query_value(value: Option<String>) -> Result<String, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .ok_or(ApiError::InvalidRequest)
}
