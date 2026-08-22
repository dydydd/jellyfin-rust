use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_data::ChapterRepository;
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

    let include_segment_types = query.include_segment_types;
    let include_unknown = include_segment_types.is_empty()
        || include_segment_types.contains(&MediaSegmentType::Unknown);
    let chapters = ChapterRepository::new(state.database.clone())
        .list_for_item(item_id)
        .await
        .map_err(|_| ApiError::Internal)?;
    let items = chapters
        .into_iter()
        .filter(|_| include_unknown)
        .map(|chapter| MediaSegmentDto {
            id: chapter.id,
            item_id: chapter.item_id,
            segment_type: MediaSegmentType::Unknown,
            start_ticks: chapter.start_position_ticks,
            end_ticks: chapter.end_position_ticks,
        })
        .collect();
    Ok(Json(QueryResult::from_items(items)))
}
