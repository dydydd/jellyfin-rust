use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_model::{MediaSegmentDto, MediaSegmentType, QueryResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MediaSegmentsQuery {
    #[serde(
        default,
        rename = "includeSegmentTypes",
        alias = "IncludeSegmentTypes",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    include_segment_types: Vec<MediaSegmentType>,
}

pub(crate) async fn get_item_segments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<MediaSegmentsQuery>,
) -> Result<Json<QueryResult<MediaSegmentDto>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;

    // Media segment providers are not wired yet. Keeping the parsed filter at
    // the API boundary preserves the official contract while returning the
    // provider aggregate's empty result.
    let _include_segment_types = query.include_segment_types;
    Ok(Json(QueryResult::from_items(Vec::new())))
}
