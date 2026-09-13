use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;

use crate::{ApiError, AppState, items, user_library};

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(mut query): Query<items::ItemsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    query.force_include_item_type("Trailer");
    let mut result = items::query_items(state, headers, query).await?;
    user_library::omit_incompatible_emby_relations(&uri, &mut result.0.items);
    Ok(result)
}
