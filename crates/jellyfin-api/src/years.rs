use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use jellyfin_controller::YearItem;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct YearQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(year): Path<i32>,
    Query(query): Query<YearQuery>,
) -> Result<Json<user_library::BaseItemDto>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let year = state
        .years
        .get(&authenticated.user, target_user_id, year)
        .await?;
    Ok(Json(match year {
        YearItem::Persisted(item) => user_library::item_to_dto(item, state.server_id()),
        YearItem::Virtual(year) => user_library::year_to_dto(&year, state.server_id()),
    }))
}
