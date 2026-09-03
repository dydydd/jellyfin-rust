use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, Query, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use jellyfin_live_tv::listings::{GuideRefreshSummary, LineupsResponse};
use jellyfin_model::{GuideInfo, TunerHostInfo};
use serde::Deserialize;
use serde_json::{Value, json};

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
    let end = start + Duration::days(7);
    Ok(Json(GuideInfo {
        start_date: start,
        end_date: end,
    }))
}

pub(crate) async fn info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let hosts = state.tuner_hosts.list().await?;
    Ok(Json(json!({
        "Services": [],
        "IsEnabled": !hosts.is_empty(),
        "EnabledUsers": []
    })))
}

pub(crate) async fn channels(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn channel(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _channel_id: Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

pub(crate) async fn recordings(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn recording_series(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn recording_groups(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn recording_folders(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn recording(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _recording_id: Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

pub(crate) async fn timers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _timer_id: Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

pub(crate) async fn timer_defaults(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(json!({
        "Type": "SeriesTimer",
        "RecordAnyTime": false,
        "SkipEpisodesInLibrary": false,
        "RecordAnyChannel": false,
        "KeepUpTo": 0,
        "RecordNewOnly": false,
        "Days": [],
        "ImageTags": {}
    })))
}

pub(crate) async fn programs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn programs_post(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn recommended_programs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn program(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _program_id: Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

pub(crate) async fn series_timers(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(empty_query_result()))
}

pub(crate) async fn series_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _timer_id: Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

pub(crate) async fn listing_provider_default(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(default_listing_provider()))
}

pub(crate) async fn listing_providers_post(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let body = body.map_err(|_| ApiError::InvalidRequest)?;
    let mut response = Json(body.0).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, JSON_UTF8);
    Ok(response)
}

pub(crate) async fn delete_listing_provider(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _query: Result<Query<DeleteTunerHostQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn schedules_direct_countries(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(json!([])))
}

pub(crate) async fn channel_mapping_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(json!({
        "TunerChannels": [],
        "ProviderChannels": [],
        "Mappings": [],
        "ProviderName": null
    })))
}

pub(crate) async fn set_channel_mapping(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let body = body.map_err(|_| ApiError::InvalidRequest)?;
    Ok(Json(body.0))
}

pub(crate) async fn tuner_host_types(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Ok(Json(json!([
        { "Name": "M3U Tuner", "Id": "m3u" },
        { "Name": "HD Homerun", "Id": "hdhomerun" }
    ])))
}

pub(crate) async fn discover_tuners(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<jellyfin_model::TunerHostInfo>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(Vec::new()))
}

pub(crate) async fn reset_tuner(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _tuner_id: Path<String>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_recording(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _recording_id: Path<String>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn cancel_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _timer_id: Path<String>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _timer_id: Path<String>,
    _body: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _body: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn cancel_series_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _timer_id: Path<String>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_series_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _timer_id: Path<String>,
    _body: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_series_timer(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _body: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn live_recording_stream(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _recording_id: Path<String>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

pub(crate) async fn live_stream_file(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    _stream: Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    Err(ApiError::NotFound)
}

fn empty_query_result() -> Value {
    json!({
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
    })
}

fn default_listing_provider() -> Value {
    json!({
        "Type": "",
        "Username": "",
        "Password": "",
        "ListingsId": "",
        "ZipCode": "",
        "Country": "",
        "Path": "",
        "EnabledTuners": [],
        "EnableAllTuners": true,
        "NewsCategories": ["news", "journalism", "documentary", "current affairs"],
        "SportsCategories": ["sports", "basketball", "baseball", "football"],
        "KidsCategories": ["kids", "family", "children", "childrens", "disney"],
        "MovieCategories": ["movie"],
        "ChannelMappings": [],
        "MoviePrefix": "",
        "PreferredLanguage": "",
        "UserAgent": ""
    })
}
