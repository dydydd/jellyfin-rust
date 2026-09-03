use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::HeaderMap,
};
use jellyfin_model::{CountryInfo, CultureDto, LocalizationOption, ParentalRating};

use crate::{ApiError, AppState, authorization};

pub(crate) async fn cultures(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<&'static [CultureDto]>, ApiError> {
    authorization::require_first_time_setup_or_default(&state, &headers, &uri).await?;
    Ok(Json(state.localization.distinct_sorted_cultures()))
}

pub(crate) async fn countries(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<&'static [CountryInfo]>, ApiError> {
    authorization::require_first_time_setup_or_default(&state, &headers, &uri).await?;
    Ok(Json(state.localization.countries()))
}

pub(crate) async fn parental_ratings(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<ParentalRating>>, ApiError> {
    authorization::require_first_time_setup_or_default(&state, &headers, &uri).await?;
    let configuration = state.server_configuration.load().await?;
    Ok(Json(
        state
            .localization
            .parental_ratings(&configuration.metadata_country_code),
    ))
}

pub(crate) async fn options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<&'static [LocalizationOption]>, ApiError> {
    authorization::require_first_time_setup_or_default(&state, &headers, &uri).await?;
    Ok(Json(state.localization.localization_options()))
}
