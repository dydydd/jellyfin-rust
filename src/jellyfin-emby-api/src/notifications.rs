//! Emby notification capability discovery.
//!
//! The Rust server does not currently expose an Emby notification provider,
//! so the truthful generated-client contract is an empty category array.

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use jellyfin_api::AppState;
use serde::Serialize;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Notifications/Types", get(notification_types))
        .route("/notifications/types", get(notification_types))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NotificationCategoryInfo {
    name: Option<String>,
    id: Option<String>,
    events: Option<Vec<NotificationTypeInfo>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NotificationTypeInfo {
    name: Option<String>,
    id: Option<String>,
    category_id: Option<String>,
    category_name: Option<String>,
}

async fn notification_types() -> Json<Vec<NotificationCategoryInfo>> {
    Json(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn notification_types_are_a_generated_client_decodable_empty_array() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));

        for path in ["/Notifications/Types", "/notifications/types"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK, "{path}");
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                serde_json::json!([])
            );
        }
    }
}
