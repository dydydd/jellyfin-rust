use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication::AuthenticatedIdentity, authorization};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CreateQuery {
    #[serde(alias = "Name")]
    name: Option<String>,
    #[serde(
        alias = "Ids",
        alias = "IDs",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<String>,
    #[serde(alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(alias = "IsLocked")]
    is_locked: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemsQuery {
    #[serde(
        default,
        alias = "Ids",
        alias = "IDs",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct CollectionCreationResult {
    id: Uuid,
}

pub(crate) async fn create(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<CreateQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<CollectionCreationResult>, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    require_collection_management(&identity)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let ids = query
        .ids
        .iter()
        .map(|id| Uuid::parse_str(id).map_err(|_| ApiError::InvalidRequest))
        .collect::<Result<Vec<_>, _>>()?;
    let id = state
        .collections
        .create(query.name, query.parent_id, query.is_locked, &ids)
        .await?;
    Ok(Json(CollectionCreationResult { id }))
}

pub(crate) async fn add_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ItemsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    require_collection_management(&identity)?;
    let Path(collection_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    if query.ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    state.collections.add(collection_id, &query.ids).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_items(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ItemsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<StatusCode, ApiError> {
    let identity = authorization::require_default(&state, &headers, &uri).await?;
    require_collection_management(&identity)?;
    let Path(collection_id) = path.map_err(|_| ApiError::InvalidRequest)?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    if query.ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    state.collections.remove(collection_id, &query.ids).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn require_collection_management(identity: &AuthenticatedIdentity) -> Result<(), ApiError> {
    match identity {
        AuthenticatedIdentity::Device(session) if !session.can_manage_collections() => {
            Err(ApiError::Forbidden)
        }
        AuthenticatedIdentity::Device(_) | AuthenticatedIdentity::ApiKey(_) => Ok(()),
    }
}
