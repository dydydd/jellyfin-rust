use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode},
};
use jellyfin_model::NameIdPair;

use crate::{ApiError, AppState, authentication};

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
