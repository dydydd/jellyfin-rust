use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use jellyfin_data::NamedConfigurationStoreError;
use serde::{Deserialize, Serialize};

use crate::{ApiError, AppState, authentication};

const BRANDING_CONFIGURATION_KEY: &str = "branding";
const JSON_UTF8: HeaderValue = HeaderValue::from_static("application/json; charset=utf-8");
const CSS_UTF8: HeaderValue = HeaderValue::from_static("text/css; charset=utf-8");

/// Complete server-side branding configuration.
///
/// `splashscreen_location` is retained for server image handling but is never
/// exposed by the public branding configuration endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BrandingOptions {
    pub login_disclaimer: Option<String>,
    pub custom_css: Option<String>,
    pub splashscreen_enabled: bool,
    pub splashscreen_location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct BrandingOptionsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) login_disclaimer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) custom_css: Option<String>,
    pub(crate) splashscreen_enabled: bool,
}

impl Default for BrandingOptionsDto {
    fn default() -> Self {
        Self {
            login_disclaimer: None,
            custom_css: None,
            splashscreen_enabled: false,
        }
    }
}

impl From<&BrandingOptions> for BrandingOptionsDto {
    fn from(branding: &BrandingOptions) -> Self {
        Self {
            login_disclaimer: branding.login_disclaimer.clone(),
            custom_css: branding.custom_css.clone(),
            splashscreen_enabled: branding.splashscreen_enabled,
        }
    }
}

pub(crate) async fn get_configuration(
    State(state): State<Arc<AppState>>,
) -> Result<
    (
        [(header::HeaderName, HeaderValue); 1],
        Json<BrandingOptionsDto>,
    ),
    ApiError,
> {
    let branding = branding_options(&state).await?;
    Ok((
        [(header::CONTENT_TYPE, JSON_UTF8)],
        Json(BrandingOptionsDto::from(&branding)),
    ))
}

pub(crate) async fn update_configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<BrandingOptionsDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    let mut branding = branding_options(&state).await?;
    branding.login_disclaimer = request.login_disclaimer;
    branding.custom_css = request.custom_css;
    branding.splashscreen_enabled = request.splashscreen_enabled;
    save_branding_options(&state, &branding).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn get_css(
    State(state): State<Arc<AppState>>,
) -> Result<([(header::HeaderName, HeaderValue); 1], String), ApiError> {
    let css = branding_options(&state)
        .await?
        .custom_css
        .unwrap_or_default();
    Ok(([(header::CONTENT_TYPE, CSS_UTF8)], css))
}

async fn branding_options(state: &AppState) -> Result<BrandingOptions, ApiError> {
    if let Some(repository) = &state.named_configurations {
        return match repository.load(BRANDING_CONFIGURATION_KEY).await {
            Ok(configuration) => {
                serde_json::from_value(configuration.configuration).map_err(|_| ApiError::Internal)
            }
            Err(NamedConfigurationStoreError::NotFound(_)) => Ok(BrandingOptions::default()),
            Err(error) => Err(error.into()),
        };
    }

    Ok(state.branding.read().await.clone())
}

async fn save_branding_options(
    state: &AppState,
    branding: &BrandingOptions,
) -> Result<(), ApiError> {
    if let Some(repository) = &state.named_configurations {
        let configuration = serde_json::to_value(branding).map_err(|_| ApiError::Internal)?;
        repository
            .save(BRANDING_CONFIGURATION_KEY, configuration)
            .await?;
    }

    *state.branding.write().await = branding.clone();
    Ok(())
}
