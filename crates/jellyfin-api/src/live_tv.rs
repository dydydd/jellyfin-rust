use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use jellyfin_live_tv::listings::{GuideRefreshSummary, LineupsResponse};
use jellyfin_model::{GuideInfo, TunerHostInfo};
use serde::Deserialize;

use crate::{ApiError, AppState, authentication, authorization};

const JSON_UTF8: HeaderValue = HeaderValue::from_static("application/json; charset=utf-8");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DeleteTunerHostQuery {
    id: Option<String>,
}

pub(crate) async fn save_tuner_host(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<TunerHostInfo>, JsonRejection>,
) -> Result<Response, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(host) = request.map_err(|_| ApiError::InvalidRequest)?;
    let host = state.tuner_hosts.save(host).await?;
    let mut response = Json(host).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, JSON_UTF8);
    Ok(response)
}

pub(crate) async fn delete_tuner_host(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<DeleteTunerHostQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let id = query.ok().and_then(|Query(query)| query.id);
    state.tuner_hosts.delete(id.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn refresh_guide(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<GuideRefreshSummary>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let service = state.live_tv_guide.as_ref().ok_or(ApiError::NotFound)?;
    Ok(Json(service.refresh().await?))
}

pub(crate) async fn listing_provider_lineups(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<LineupsResponse>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let service = state.live_tv_guide.as_ref().ok_or(ApiError::NotFound)?;
    Ok(Json(service.lineups().await?))
}

pub(crate) async fn guide_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<GuideInfo>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let start = Utc::now();
    let end = start + Duration::days(3);
    Ok(Json(GuideInfo { start_date: start, end_date: end }))
}
