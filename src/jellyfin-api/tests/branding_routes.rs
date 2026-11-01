use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::{AppState, BrandingOptions};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tower::ServiceExt;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn default_branding_routes_match_the_official_contract() {
    let app = branding_app(BrandingOptions::default());

    let response = get(&app, "/Branding/Configuration").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let configuration = body_json(response).await;
    assert_eq!(configuration, json!({ "SplashscreenEnabled": false }));
    assert_eq!(
        serde_json::from_value::<BrandingOptions>(configuration).unwrap(),
        BrandingOptions::default()
    );

    for route in ["/Branding/Css", "/Branding/Css.css"] {
        let response = get(&app, route).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert_eq!(body_text(response).await, "");
    }
}

#[tokio::test]
async fn configured_branding_is_projected_without_leaking_server_paths() {
    let login_disclaimer = "欢迎使用 <strong>Jellyfin</strong>";
    let custom_css = "body::before { content: \"你好，Jellyfin\"; }\n";
    let app = branding_app(BrandingOptions {
        login_disclaimer: Some(login_disclaimer.to_owned()),
        custom_css: Some(custom_css.to_owned()),
        splashscreen_enabled: true,
        splashscreen_location: Some("/srv/jellyfin/private/splash.png".to_owned()),
    });

    let response = get(&app, "/Branding/Configuration").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let configuration = body_json(response).await;
    assert_eq!(configuration["LoginDisclaimer"], login_disclaimer);
    assert_eq!(configuration["CustomCss"], custom_css);
    assert_eq!(configuration["SplashscreenEnabled"], true);
    assert!(configuration.get("SplashscreenLocation").is_none());
    assert!(configuration.get("custom_css").is_none());

    for route in ["/Branding/Css", "/Branding/Css.css"] {
        let response = get(&app, route).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/css; charset=utf-8"
        );
        assert_eq!(body_text(response).await, custom_css);
    }
}

fn branding_app(options: BrandingOptions) -> axum::Router {
    jellyfin_api::router(
        AppState::new(
            DatabaseConnection::Disconnected,
            "Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_branding_options(options),
    )
}

async fn get(app: &axum::Router, route: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::get(route).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
