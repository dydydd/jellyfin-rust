use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use sea_orm::DatabaseConnection;
use tower::ServiceExt;

const ROBOTS_LOCATION: &str = "web/robots.txt";

#[tokio::test]
async fn official_lowercase_robots_path_redirects() {
    let response = request(&app(), Method::GET, "/robots.txt").await;

    assert_redirect(&response);
}

#[tokio::test]
async fn robots_path_is_case_insensitive_and_ignores_the_query() {
    let app = app();
    for uri in [
        "/RoBoTs.TxT",
        "/ROBOTS.TXT",
        "/robots.txt?source=integration-test",
        "/RoBoTs.TxT?source=integration-test",
    ] {
        let response = request(&app, Method::GET, uri).await;
        assert_redirect(&response);
    }
}

#[tokio::test]
async fn every_http_method_matches_the_official_middleware() {
    let app = app();
    for method in [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
        Method::TRACE,
        Method::CONNECT,
    ] {
        let response = request(&app, method.clone(), "/ROBOTS.TXT").await;
        assert_eq!(response.status(), StatusCode::FOUND, "{method}");
        assert_eq!(response.headers()[header::LOCATION], ROBOTS_LOCATION);
    }
}

#[tokio::test]
async fn fallback_authenticates_unknown_routes_before_not_found() {
    let app = app();

    for (method, uri) in [
        (Method::GET, "/not-a-route"),
        (Method::POST, "/still-not-a-route"),
    ] {
        let response = request(&app, method, uri).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(header::LOCATION));
    }

    let response = request(&app, Method::DELETE, "/System/Ping").await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(!response.headers().contains_key(header::LOCATION));
}

fn app() -> axum::Router {
    jellyfin_api::router(AppState::new(
        DatabaseConnection::Disconnected,
        "Robots Redirect Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ))
}

async fn request(app: &axum::Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn assert_redirect(response: &axum::response::Response) {
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], ROBOTS_LOCATION);
}
