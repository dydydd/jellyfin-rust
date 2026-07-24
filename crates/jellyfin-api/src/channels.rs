use std::sync::Arc;

use axum::{Json, extract::State, http::HeaderMap};
use axum_extra::extract::Query;
use jellyfin_data::{BaseItemOrder, BaseItemQuery};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChannelsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
    #[serde(rename = "supportsLatestItems", alias = "SupportsLatestItems")]
    supports_latest_items: Option<bool>,
    #[serde(rename = "supportsMediaDeletion", alias = "SupportsMediaDeletion")]
    supports_media_deletion: Option<bool>,
    #[serde(rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ChannelsQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let _ = (
        query.supports_latest_items,
        query.supports_media_deletion,
        query.is_favorite,
    );
    let page = state
        .user_library
        .query_items(
            &authenticated.user,
            target_user_id,
            BaseItemQuery {
                recursive: true,
                include_item_types: vec!["Channel".to_owned()],
                order: BaseItemOrder::SortName,
                start_index: query.start_index,
                limit: query.limit,
                enable_total_record_count: Some(true),
                ..BaseItemQuery::default()
            },
        )
        .await?;
    let items = page
        .items
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
        items,
    }))
}
