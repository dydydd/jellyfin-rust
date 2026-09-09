//! Emby API surface for Android and iOS clients.
//!
//! The generated clients in `Emby.ApiClients` identify themselves with either
//! the `Emby` or `MediaBrowser` scheme and use Emby's `/emby` API base path.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;

mod auth_user;
mod encoding;
mod library;
mod system_misc;
mod users;

/// Emby's Android and iOS API base path.
pub const EMBY_API_PREFIX: &str = "/emby";

/// Version advertised by the checked-in Emby client contract.
const EMBY_API_VERSION: &str = "4.9.5.0";

/// Builds the independent Emby route tree.
pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let fallback = jellyfin_api::unprefixed_router(state.as_ref().clone());
    let routes = Router::new()
        .merge(auth_user::routes())
        .merge(encoding::routes())
        .merge(library::routes())
        .merge(system_misc::routes())
        .merge(users::routes())
        .route("/Branding/Configuration", get(branding_configuration))
        .route("/branding/configuration", get(branding_configuration))
        .route("/System/Info/Public", get(public_system_info))
        .route("/system/info/public", get(public_system_info))
        .route("/System/Info", get(system_info))
        .route("/system/info", get(system_info))
        .fallback_service(fallback)
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            jellyfin_api::protocol_route_auth,
        ))
        .with_state(state);

    Router::new().nest(EMBY_API_PREFIX, routes)
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    local_address: Option<String>,
    local_addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wan_address: Option<String>,
    remote_addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct BrandingOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    login_disclaimer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_css: Option<String>,
}

impl From<jellyfin_model::PublicSystemInfo> for PublicSystemInfo {
    fn from(info: jellyfin_model::PublicSystemInfo) -> Self {
        Self {
            local_addresses: info.local_address.iter().cloned().collect(),
            local_address: info.local_address,
            wan_address: None,
            remote_addresses: Vec::new(),
            server_name: info.server_name,
            version: Some(EMBY_API_VERSION.to_owned()),
            id: info.id,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SystemInfo {
    #[serde(flatten)]
    public_info: PublicSystemInfo,
    operating_system_display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_name: Option<String>,
    has_pending_restart: bool,
    is_shutting_down: bool,
    operating_system: String,
    supports_library_monitor: bool,
    web_socket_port_number: i32,
    completed_installations: Vec<()>,
    can_self_restart: bool,
    can_launch_web_browser: bool,
    program_data_path: String,
    items_by_name_path: String,
    cache_path: String,
    log_path: String,
    internal_metadata_path: String,
    transcoding_temp_path: String,
    has_update_available: bool,
}

async fn public_system_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicSystemInfo>, StatusCode> {
    Ok(Json(state.public_system_info().await?.into()))
}

async fn branding_configuration(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BrandingOptions>, StatusCode> {
    let options = state.branding_options().await?;
    Ok(Json(BrandingOptions {
        login_disclaimer: options.login_disclaimer,
        custom_css: options.custom_css,
    }))
}

async fn system_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<SystemInfo>, Response> {
    let info = state.system_info(&headers, &uri).await?;
    let public_info = PublicSystemInfo::from(info.public_info.clone());
    Ok(Json(SystemInfo {
        public_info,
        operating_system_display_name: info.operating_system_display_name,
        package_name: info.package_name,
        has_pending_restart: info.has_pending_restart,
        is_shutting_down: info.is_shutting_down,
        operating_system: info.public_info.operating_system,
        supports_library_monitor: info.supports_library_monitor,
        web_socket_port_number: info.web_socket_port_number,
        completed_installations: Vec::new(),
        can_self_restart: info.can_self_restart,
        can_launch_web_browser: info.can_launch_web_browser,
        program_data_path: info.program_data_path,
        items_by_name_path: info.items_by_name_path,
        cache_path: info.cache_path,
        log_path: info.log_path,
        internal_metadata_path: info.internal_metadata_path,
        transcoding_temp_path: info.transcoding_temp_path,
        has_update_available: info.has_update_available,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn jellyfin_and_emby_are_separate_route_trees() {
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let jellyfin = jellyfin_api::router(state.clone());
        let emby = router(state);

        assert_eq!(status(&jellyfin, "/GetUtcTime").await, StatusCode::OK);
        assert_eq!(status(&jellyfin, "/api/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&jellyfin, "/emby/GetUtcTime").await, StatusCode::OK);

        assert_eq!(status(&emby, "/emby/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&emby, "/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&emby, "/api/GetUtcTime").await, StatusCode::OK);

        let jellyfin_info = body(&jellyfin, "/System/Info/Public").await;
        let emby_info = body(&emby, "/emby/System/Info/Public").await;
        assert!(jellyfin_info.get("ProductName").is_some());
        assert!(emby_info.get("ProductName").is_none());
        assert_eq!(emby_info["Version"], EMBY_API_VERSION);
        assert_eq!(emby_info["LocalAddresses"][0], "http://127.0.0.1:8096");
        assert!(emby_info["RemoteAddresses"].is_array());

        let jellyfin_info = body(&jellyfin, "/System/Info").await;
        let emby_info = body(&emby, "/emby/System/Info").await;
        assert!(jellyfin_info.get("WebPath").is_some());
        assert!(emby_info.get("WebPath").is_none());
        assert!(emby_info.get("LocalAddresses").is_some());
        assert!(emby_info.get("CompletedInstallations").is_some());

        let jellyfin_branding = body(&jellyfin, "/Branding/Configuration").await;
        let emby_branding = body(&emby, "/emby/Branding/Configuration").await;
        assert!(jellyfin_branding.get("SplashscreenEnabled").is_some());
        assert!(emby_branding.get("SplashscreenEnabled").is_none());
    }

    async fn status(app: &Router, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    async fn body(app: &Router, uri: &str) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
    }
}
