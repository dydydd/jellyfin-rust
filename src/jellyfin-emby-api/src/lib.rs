//! Emby API surface for Android and iOS clients.
//!
//! The generated clients in `Emby.ApiClients` identify themselves with either
//! the `Emby` or `MediaBrowser` scheme and use Emby's `/emby` API base path.

use axum::Router;
use jellyfin_api::AppState;

/// Emby's Android and iOS API base path.
pub const EMBY_API_PREFIX: &str = "/emby";

/// Builds the independent Emby route tree.
pub fn router(state: AppState) -> Router {
    Router::new().nest(EMBY_API_PREFIX, jellyfin_api::unprefixed_router(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
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
    }

    async fn status(app: &Router, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }
}
