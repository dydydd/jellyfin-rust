use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::Query;
use jellyfin_model::{MediaSegmentDto, MediaSegmentType, QueryResult};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MediaSegmentsQuery {
    #[serde(
        default,
        rename = "includeSegmentTypes",
        alias = "IncludeSegmentTypes",
        alias = "includesegmenttypes",
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
    if item_id.is_nil() {
        return Err(ApiError::InvalidRequest);
    }
    let item = state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    let disabled_provider_names = disabled_provider_names(&state, &item).await?;

    let items = state
        .media_segments
        .list(
            item_id,
            &query.include_segment_types,
            &disabled_provider_names,
        )
        .await?;
    Ok(Json(QueryResult::from_items(items)))
}

async fn disabled_provider_names(
    state: &AppState,
    item: &jellyfin_data::entities::base_item::Model,
) -> Result<Vec<String>, ApiError> {
    let collection_folder_id = if is_collection_folder(&item.item_type) {
        Some(item.id)
    } else {
        state
            .base_items
            .nearest_ancestor_ids_by_type(&[item.id], &["CollectionFolder".to_owned()])
            .await?
            .remove(&item.id)
    };
    let Some(collection_folder_id) = collection_folder_id else {
        return Ok(Vec::new());
    };
    let options = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .find(|folder| folder.id == collection_folder_id)
        .map_or(Value::Null, |folder| folder.library_options);
    Ok(string_array_option(
        &options,
        "DisabledMediaSegmentProviders",
    ))
}

fn is_collection_folder(item_type: &str) -> bool {
    item_type
        .rsplit('.')
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("CollectionFolder"))
}

fn string_array_option(options: &Value, name: &str) -> Vec<String> {
    options
        .as_object()
        .and_then(|options| {
            options
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
        })
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
