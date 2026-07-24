use std::sync::Arc;

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::Query;

use crate::{ApiError, AppState, items, user_library};

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(mut query): Query<items::ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    query.force_include_item_type("Trailer");
    items::query_items(state, headers, query).await
}
