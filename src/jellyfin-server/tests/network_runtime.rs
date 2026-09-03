use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware,
    response::IntoResponse,
    routing::get,
};
use jellyfin_networking::{NetworkConfiguration, NetworkManager};
use jellyfin_server::{
    ForwardedRequestInfo, HostNetworkConfigurationError, apply_forwarded_headers, cors_layer,
    validate_tls_configuration,
};
use tower::ServiceExt;

#[test]
fn cors_policy_matches_official_wildcard_and_explicit_modes() {
    assert!(cors_layer(&[]).is_ok());
    assert!(cors_layer(&["*".to_owned()]).is_ok());
    assert!(cors_layer(&["https://app.example".to_owned()]).is_ok());
    assert!(matches!(
        cors_layer(&["*".to_owned(), "https://app.example".to_owned()]),
        Err(HostNetworkConfigurationError::MixedWildcardCorsOrigins)
    ));
}

#[test]
fn tls_configuration_fails_explicitly_when_native_tls_is_unavailable() {
    let mut config = NetworkConfiguration::default();
    assert!(validate_tls_configuration(&config).is_ok());
    config.enable_https = true;
    assert_eq!(
        validate_tls_configuration(&config),
        Err(HostNetworkConfigurationError::TlsUnsupported)
    );
}

#[tokio::test]
async fn forwarded_headers_are_used_only_from_trusted_proxy() {
    async fn handler(
        State(seen): State<Arc<tokio::sync::Mutex<Option<IpAddr>>>>,
        request: axum::extract::Request,
    ) -> impl IntoResponse {
        let ip = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .unwrap()
            .0
            .ip();
        *seen.lock().await = Some(ip);
        StatusCode::NO_CONTENT
    }
    let mut config = NetworkConfiguration::default();
    config.known_proxies = vec!["10.0.0.1".to_owned()];
    let network = Arc::new(NetworkManager::new(config, Vec::new()));
    let seen = Arc::new(tokio::sync::Mutex::new(None));
    let app = Router::new()
        .route("/", get(handler))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&network),
            apply_forwarded_headers,
        ))
        .with_state(Arc::clone(&seen));

    let mut request = Request::builder()
        .uri("/")
        .header("x-forwarded-for", "192.0.2.7")
        .body(axum::body::Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        Ipv4Addr::new(10, 0, 0, 1).into(),
        1234,
    )));
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(*seen.lock().await, Some(Ipv4Addr::new(192, 0, 2, 7).into()));

    let mut request = Request::builder()
        .uri("/")
        .header("x-forwarded-for", "192.0.2.7")
        .body(axum::body::Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        Ipv4Addr::new(10, 0, 0, 2).into(),
        1234,
    )));
    app.oneshot(request).await.unwrap();
    assert_eq!(*seen.lock().await, Some(Ipv4Addr::new(10, 0, 0, 2).into()));
}

#[test]
fn forwarded_info_type_is_available_to_handlers() {
    assert_eq!(ForwardedRequestInfo::default().proto, None);
}
