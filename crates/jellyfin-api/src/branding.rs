use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderValue, header},
};
use serde::{Deserialize, Serialize};

use crate::AppState;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct BrandingOptionsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    login_disclaimer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_css: Option<String>,
    splashscreen_enabled: bool,
}

pub(crate) async fn get_configuration(
    State(state): State<Arc<AppState>>,
) -> (
    [(header::HeaderName, HeaderValue); 1],
    Json<BrandingOptionsDto>,
) {
    let branding = state.branding.read().await;
    let configuration = BrandingOptionsDto {
        login_disclaimer: branding.login_disclaimer.clone(),
        custom_css: branding.custom_css.clone(),
        splashscreen_enabled: branding.splashscreen_enabled,
    };
    ([(header::CONTENT_TYPE, JSON_UTF8)], Json(configuration))
}

pub(crate) async fn get_css(
    State(state): State<Arc<AppState>>,
) -> ([(header::HeaderName, HeaderValue); 1], String) {
    let css = state
        .branding
        .read()
        .await
        .custom_css
        .clone()
        .unwrap_or_default();
    ([(header::CONTENT_TYPE, CSS_UTF8)], css)
}
