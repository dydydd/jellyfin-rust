use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use jellyfin_api::AppState;
use sea_orm::DatabaseConnection;
use tower::ServiceExt;

const MAX_BODY_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn jellyfin_root_and_api_prefix_serve_the_same_routes() {
    let app = app();

    for prefix in ["", "/api"] {
        let response = get(&app, &format!("{prefix}/api-docs/openapi.json")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["info"]["title"], "Jellyfin API");

        let response = get(&app, &format!("{prefix}/GetUtcTime")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["RequestReceptionTime"].is_string());

        for path in [
            "/System/Info/Public",
            "/Branding/Configuration",
            "/Branding/Css",
            "/Branding/Css.css",
        ] {
            assert_eq!(
                request(&app, Method::GET, &format!("{prefix}{path}"))
                    .await
                    .status(),
                StatusCode::OK,
                "anonymous Jellyfin bootstrap route {prefix}{path}",
            );
        }
        for method in [Method::GET, Method::POST, Method::HEAD] {
            assert_eq!(
                request(&app, method.clone(), &format!("{prefix}/System/Ping"))
                    .await
                    .status(),
                StatusCode::OK,
                "anonymous Jellyfin ping route {method} {prefix}/System/Ping",
            );
        }
    }
}

#[tokio::test]
async fn prefixed_unknown_api_routes_fail_closed() {
    let app = app();

    for uri in ["/api/not-a-route", "/api/System/Logs"] {
        assert_eq!(
            get(&app, uri).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
}

fn app() -> Router {
    jellyfin_api::router(AppState::new(
        DatabaseConnection::Disconnected,
        "Compat Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ))
}

async fn get(app: &Router, uri: &str) -> axum::response::Response {
    request(app, Method::GET, uri).await
}

async fn request(app: &Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("router must respond")
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), MAX_BODY_SIZE)
        .await
        .expect("response body must be readable");
    serde_json::from_slice(&bytes).expect("response body must contain JSON")
}
