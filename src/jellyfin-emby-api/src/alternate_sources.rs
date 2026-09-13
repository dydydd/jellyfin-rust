//! Emby's legacy POST alias for deleting alternate video relationships.

use std::sync::Arc;

use axum::{
    Router,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use jellyfin_api::AppState;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Videos/{item_id}/AlternateSources/Delete",
            post(delete_alternate_sources),
        )
        .route(
            "/videos/{item_id}/alternatesources/delete",
            post(delete_alternate_sources),
        )
}

#[allow(clippy::result_large_err)]
async fn delete_alternate_sources(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<String>,
) -> Result<StatusCode, Response> {
    state
        .delete_emby_alternate_sources_for_request(&headers, &uri, &item_id)
        .await?;
    Ok(StatusCode::OK)
}
