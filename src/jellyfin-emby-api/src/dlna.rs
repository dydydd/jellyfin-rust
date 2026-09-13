//! Emby DLNA profile discovery.
//!
//! Playback accepts client-supplied device profiles, but the Rust server does
//! not register Emby's persistent DLNA profile provider.  Its administrator
//! discovery endpoint therefore exposes the generated client's valid empty
//! profile collection.  Profile mutations and UPnP server routes remain
//! unavailable rather than pretending that a profile is operational.

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use jellyfin_api::AppState;
use serde_json::Value;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Dlna/ProfileInfos", get(profile_infos))
        .route("/dlna/profileinfos", get(profile_infos))
}

async fn profile_infos() -> Json<Vec<Value>> {
    Json(Vec::new())
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
    async fn profile_infos_are_a_generated_client_decodable_empty_array() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        for path in ["/Dlna/ProfileInfos", "/dlna/profileinfos"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                serde_json::json!([])
            );
        }
    }
}
