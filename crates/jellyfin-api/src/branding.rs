use std::{path::PathBuf, sync::Arc};

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::Query;
use chrono::{DateTime, Utc};
use jellyfin_data::NamedConfigurationStoreError;
use jellyfin_model::MimeTypes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication,
    item_images::{decode_base64_image_body, render_simple_image},
};

const BRANDING_CONFIGURATION_KEY: &str = "branding";
const JSON_UTF8: HeaderValue = HeaderValue::from_static("application/json; charset=utf-8");
const CSS_UTF8: HeaderValue = HeaderValue::from_static("text/css; charset=utf-8");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SplashscreenQuery {
    #[serde(default, rename = "tag", alias = "Tag")]
    tag: Option<String>,
    #[serde(default, rename = "format", alias = "Format")]
    format: Option<String>,
}

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
#[derive(Default)]
pub(crate) struct BrandingOptionsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) login_disclaimer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) custom_css: Option<String>,
    pub(crate) splashscreen_enabled: bool,
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

pub(crate) async fn get_splashscreen(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<SplashscreenQuery>,
) -> Result<Response, ApiError> {
    let identity = authentication::optional_authenticated_identity(&state, &headers, &uri).await?;
    let branding = branding_options(&state).await?;
    if !branding.splashscreen_enabled
        && !identity.is_some_and(|identity| identity.is_administrator_equivalent())
    {
        return Err(ApiError::NotFound);
    }
    let path = resolve_splashscreen_path(&state, &branding)
        .await
        .ok_or(ApiError::NotFound)?;
    let modified = tokio::fs::metadata(&path)
        .await
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .map_err(|_| ApiError::Internal)?;
    render_simple_image(
        &state,
        &headers,
        path,
        modified,
        query.tag.as_deref(),
        query.format.as_deref(),
        90,
    )
    .await
}

pub(crate) async fn upload_splashscreen(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: axum::http::Request<Body>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let extension = MimeTypes::try_get_image_extension(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
    )
    .ok_or(ApiError::InvalidRequest)?;
    let image = decode_base64_image_body(request.into_body()).await?;
    let mut branding = branding_options(&state).await?;
    tokio::fs::create_dir_all(&state.program_data_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let target = state
        .program_data_directory
        .join(format!("splashscreen-upload{extension}"));
    let temporary = state.program_data_directory.join(format!(
        ".splashscreen-upload-{}.tmp",
        Uuid::new_v4().simple()
    ));
    let backup = if tokio::fs::try_exists(&target).await.unwrap_or(false) {
        let backup = state.program_data_directory.join(format!(
            ".splashscreen-upload-{}.backup",
            Uuid::new_v4().simple()
        ));
        tokio::fs::copy(&target, &backup)
            .await
            .map_err(|_| ApiError::Internal)?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = write_atomic(&temporary, &target, &image).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if let Some(backup) = backup {
            let _ = tokio::fs::remove_file(backup).await;
        }
        return Err(error);
    }
    branding.splashscreen_location = Some(target.to_string_lossy().into_owned());
    if let Err(error) = save_branding_options(&state, &branding).await {
        if let Some(backup) = backup {
            let _ = tokio::fs::rename(backup, &target).await;
        } else {
            let _ = tokio::fs::remove_file(&target).await;
        }
        return Err(error);
    }
    if let Some(backup) = backup {
        let _ = tokio::fs::remove_file(backup).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_splashscreen(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let mut branding = branding_options(&state).await?;
    let Some(path) = branding.splashscreen_location.as_deref() else {
        return Ok(StatusCode::NO_CONTENT);
    };
    match tokio::fs::remove_file(path).await {
        Ok(()) => {
            branding.splashscreen_location = None;
            save_branding_options(&state, &branding).await?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ApiError::Internal),
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_splashscreen_path(
    state: &AppState,
    branding: &BrandingOptions,
) -> Option<PathBuf> {
    if let Some(path) = branding
        .splashscreen_location
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        && tokio::fs::try_exists(&path).await.unwrap_or(false)
    {
        return Some(path);
    }
    let fallback = state.program_data_directory.join("splashscreen.png");
    tokio::fs::try_exists(&fallback)
        .await
        .unwrap_or(false)
        .then_some(fallback)
}

async fn write_atomic(
    temporary: &std::path::Path,
    target: &std::path::Path,
    bytes: &[u8],
) -> Result<(), ApiError> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(temporary)
        .await
        .map_err(|_| ApiError::Internal)?;
    file.write_all(bytes)
        .await
        .map_err(|_| ApiError::Internal)?;
    file.sync_all().await.map_err(|_| ApiError::Internal)?;
    drop(file);
    tokio::fs::rename(temporary, target)
        .await
        .map_err(|_| ApiError::Internal)
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
