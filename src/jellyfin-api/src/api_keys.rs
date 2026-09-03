use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::entities::api_key;
use jellyfin_model::{AuthenticationInfo, QueryResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CreateKeyQuery {
    app: Option<String>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<QueryResult<AuthenticationInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let keys = state.api_keys.list().await?;
    Ok(Json(QueryResult::from_items(
        keys.into_iter()
            .map(api_key_to_authentication_info)
            .collect(),
    )))
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<CreateKeyQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let app = query
        .app
        .as_deref()
        .map(str::trim)
        .filter(|app| !app.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    state.api_keys.create(app).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn revoke(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    state.api_keys.revoke(&key).await?;
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

fn api_key_to_authentication_info(key: api_key::Model) -> AuthenticationInfo {
    AuthenticationInfo {
        id: key.id,
        access_token: key.access_token,
        device_id: None,
        app_name: key.name,
        app_version: None,
        device_name: None,
        user_id: Uuid::nil(),
        is_active: true,
        date_created: key.date_created,
        date_revoked: None,
        date_last_activity: key.date_last_activity,
        user_name: None,
    }
}
