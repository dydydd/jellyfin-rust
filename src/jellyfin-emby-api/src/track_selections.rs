//! Emby generated-client routes for clearing remembered stream selections.

use std::sync::Arc;

use axum::{
    Router,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{delete, post},
};
use jellyfin_api::AppState;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Users/{user_id}/TrackSelections/{track_type}",
            delete(clear),
        )
        .route(
            "/users/{user_id}/trackselections/{track_type}",
            delete(clear),
        )
        .route(
            "/Users/{user_id}/TrackSelections/{track_type}/Delete",
            post(clear),
        )
        .route(
            "/users/{user_id}/trackselections/{track_type}/delete",
            post(clear),
        )
}

#[allow(clippy::result_large_err)]
async fn clear(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, track_type)): Path<(String, String)>,
) -> Result<StatusCode, Response> {
    state
        .clear_emby_track_selections_for_request(&headers, &uri, &user_id, &track_type)
        .await
}
