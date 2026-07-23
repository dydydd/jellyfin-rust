use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::HeaderMap,
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

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}
