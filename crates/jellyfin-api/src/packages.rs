use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
};
use jellyfin_model::{PackageInfo, RepositoryInfo};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PackageQuery {
    #[serde(rename = "assemblyGuid", alias = "AssemblyGuid")]
    assembly_guid: Option<Uuid>,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<PackageInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.packages.list()))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<PackageQuery>,
) -> Result<Json<PackageInfo>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.packages.get(&name, query.assembly_guid)?))
}

pub(crate) async fn repositories(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<RepositoryInfo>>, ApiError> {
    require_elevated(&state, &headers, &uri).await?;
    Ok(Json(state.packages.repositories()))
}

async fn require_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}
