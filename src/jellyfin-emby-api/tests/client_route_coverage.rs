use axum::{
    Router,
    body::Body,
    extract::{MatchedPath, Request},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use jellyfin_api::AppState;
use sea_orm::DatabaseConnection;
use tower::ServiceExt;

const MATCHED_PATH_HEADER: HeaderName = HeaderName::from_static("x-test-matched-path");

fn combined_router() -> Router {
    let state = AppState::new(
        DatabaseConnection::Disconnected,
        "API Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    );
    jellyfin_api::router(state.clone())
        .merge(jellyfin_emby_api::router(state))
        .layer(middleware::from_fn(expose_matched_path))
}

async fn expose_matched_path(request: Request, next: Next) -> Response {
    let matched_path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let mut response = next.run(request).await;
    if let Some(path) = matched_path {
        response.headers_mut().insert(
            MATCHED_PATH_HEADER,
            HeaderValue::from_str(&path).expect("matched paths are valid header values"),
        );
    }
    response
}

async fn response(path: &str) -> Response {
    combined_router()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn combined_server_keeps_protocol_routes_separate() {
    let jellyfin = response("/System/Info/Public").await;
    assert_eq!(jellyfin.status(), StatusCode::OK);
    assert_eq!(
        jellyfin.headers()[&MATCHED_PATH_HEADER],
        "/System/Info/Public"
    );

    let emby = response("/emby/System/Info/Public").await;
    assert_eq!(emby.status(), StatusCode::OK);
    assert_eq!(
        emby.headers()[&MATCHED_PATH_HEADER],
        "/emby/System/Info/Public"
    );

    let missing = response("/emby/ThisRouteDoesNotExist").await;
    // The Emby protocol middleware intentionally authenticates unknown paths
    // before the nested fallback decides that no handler exists.
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert!(missing.headers().get(&MATCHED_PATH_HEADER).is_none());
}
