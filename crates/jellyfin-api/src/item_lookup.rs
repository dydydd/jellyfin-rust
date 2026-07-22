use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
};
use jellyfin_model::ExternalIdInfo;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

pub(crate) async fn external_id_infos(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<ExternalIdInfo>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(state.item_lookup.external_id_infos(item_id).await?))
}
