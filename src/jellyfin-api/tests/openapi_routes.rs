use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tower::ServiceExt;

const MAX_OPENAPI_SIZE: usize = 1024 * 1024;

// Maps Jellyfin.Server.Integration.Tests/OpenApiSpecTests.GetSpec_ReturnsCorrectResponse.
#[tokio::test]
async fn official_openapi_route_returns_cached_utf8_json() {
    let app = app();

    let first = request(&app, Method::GET, "/api-docs/openapi.json").await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        first.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let first_body = to_bytes(first.into_body(), MAX_OPENAPI_SIZE)
        .await
        .expect("OpenAPI response body must be readable");
    assert!(!first_body.is_empty());
    let text = std::str::from_utf8(&first_body).expect("OpenAPI response must be UTF-8");
    oas3::from_json(text).expect("OpenAPI response must be a valid 3.1 document");

    let second = request(&app, Method::GET, "/api-docs/openapi.json").await;
    let second_body = to_bytes(second.into_body(), MAX_OPENAPI_SIZE)
        .await
        .expect("cached OpenAPI response body must be readable");
    assert_eq!(first_body, second_body);
}

#[tokio::test]
async fn openapi_document_describes_the_real_public_system_slice() {
    let response = request(&app(), Method::GET, "/api-docs/openapi.json").await;
    let body = to_bytes(response.into_body(), MAX_OPENAPI_SIZE)
        .await
        .expect("OpenAPI response body must be readable");
    let document: Value =
        serde_json::from_slice(&body).expect("OpenAPI response must contain JSON");

    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(document["info"]["title"], "Jellyfin API");
    assert_eq!(document["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        document["info"]["x-jellyfin-version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(document["x-jellyfin-partial"], true);

    let paths = document["paths"]
        .as_object()
        .expect("OpenAPI paths must be an object");
    assert!(
        paths.len() >= 300,
        "OpenAPI should inventory the real API routes"
    );
    assert_operation(&document, "/health", "get", "GetHealth");
    assert_operation(
        &document,
        "/System/Info/Public",
        "get",
        "GetPublicSystemInfo",
    );
    assert_operation(&document, "/System/Ping", "get", "GetPingSystem");
    assert_operation(&document, "/System/Ping", "post", "PostPingSystem");
    assert_operation(&document, "/api-docs/openapi.json", "get", "GetOpenApiSpec");
    assert_operation(
        &document,
        "/Users/AuthenticateByName",
        "post",
        "postUsersAuthenticateByName",
    );
    assert_operation(
        &document,
        "/Items/{item_id}/Download",
        "get",
        "getItems_item_id_Download",
    );
    assert_operation(&document, "/Sessions", "get", "getSessions");

    assert_eq!(
        document["paths"]["/System/Info/Public"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/PublicSystemInfo"
    );
    assert_eq!(
        document["paths"]["/System/Info/Public"]["get"]["responses"]["500"]["content"]["application/json"]
            ["schema"]["$ref"],
        "#/components/schemas/ApiErrorResponse"
    );
    assert!(
        document["paths"]["/System/Ping"]["get"]["responses"]["200"]["content"]
            ["text/plain; charset=utf-8"]
            .is_object()
    );
    assert_eq!(
        document["paths"]["/System/Ping"]["get"]["responses"]["200"]["content"]["text/plain; charset=utf-8"]
            ["schema"]["type"],
        "string"
    );
    assert_eq!(
        document["paths"]["/api-docs/openapi.json"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["type"],
        "object"
    );
    assert!(
        document["components"]["schemas"]["PublicSystemInfo"]["properties"]["ServerName"]
            .is_object()
    );
    assert!(
        document["components"]["schemas"]["PublicSystemInfo"]["properties"]
            ["StartupWizardCompleted"]
            .is_object()
    );
    assert!(
        document["components"]["schemas"]["PublicSystemInfo"]["properties"]
            .get("server_name")
            .is_none()
    );
    assert_eq!(
        document["components"]["schemas"]["ApiErrorResponse"]["properties"]["Message"]["type"],
        "string"
    );

    let security = &document["components"]["securitySchemes"]["CustomAuthentication"];
    assert_eq!(security["type"], "apiKey");
    assert_eq!(security["in"], "header");
    assert_eq!(security["name"], "Authorization");
    assert_eq!(security["description"], "API key header parameter");
}

#[tokio::test]
async fn openapi_route_preserves_method_and_fallback_semantics() {
    let app = app();

    let response = request(&app, Method::POST, "/api-docs/openapi.json").await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    let response = request(&app, Method::GET, "/api-docs/missing.json").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = request(&app, Method::DELETE, "/System/Ping").await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

fn app() -> axum::Router {
    jellyfin_api::router(AppState::new(
        DatabaseConnection::Disconnected,
        "OpenAPI Test Server".to_owned(),
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
                .expect("test request must be valid"),
        )
        .await
        .expect("OpenAPI router must respond")
}

fn assert_operation(document: &Value, path: &str, method: &str, operation_id: &str) {
    let operation = &document["paths"][path][method];
    assert_eq!(operation["operationId"], operation_id);
    let responses = operation["responses"]
        .as_object()
        .expect("every documented operation must define responses");
    assert!(!responses.is_empty());
    assert!(responses.values().all(|response| {
        response["description"]
            .as_str()
            .is_some_and(|description| !description.is_empty())
    }));
}
