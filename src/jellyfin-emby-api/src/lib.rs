//! Emby API surface for Android and iOS clients.
//!
//! The generated clients in `Emby.ApiClients` identify themselves with either
//! the `Emby` or `MediaBrowser` scheme and use Emby's `/emby` API base path.

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use jellyfin_api::AppState;
use serde::Serialize;

/// Emby's Android and iOS API base path.
pub const EMBY_API_PREFIX: &str = "/emby";

/// Builds the independent Emby route tree.
pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let fallback = jellyfin_api::unprefixed_router(state.as_ref().clone());
    let routes = Router::new()
        .route("/System/Info/Public", get(public_system_info))
        .route("/system/info/public", get(public_system_info))
        .fallback_service(fallback)
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

async fn public_system_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicSystemInfo>, StatusCode> {
    let info = state.public_system_info().await?;
    Ok(Json(PublicSystemInfo {
        local_addresses: info.local_address.iter().cloned().collect(),
        local_address: info.local_address,
        wan_address: None,
        remote_addresses: Vec::new(),
        server_name: info.server_name,
        version: info.version,
        id: info.id,
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
        assert_eq!(emby_info["LocalAddresses"][0], "http://127.0.0.1:8096");
        assert!(emby_info["RemoteAddresses"].is_array());
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
