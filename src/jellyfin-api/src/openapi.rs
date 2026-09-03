use std::sync::Arc;

use aide::{
    axum::{ApiRouter, routing::get_with},
    openapi::{
        ApiKeyLocation, Components, Info, OpenApi, ReferenceOr, SchemaObject, SecurityScheme,
    },
    transform::TransformResponse,
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Extension, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use indexmap::IndexMap;
use jellyfin_model::PublicSystemInfo;
use schemars::json_schema;
use serde_json::{Value, json};

use crate::AppState;

const CUSTOM_AUTHENTICATION: &str = "CustomAuthentication";
const OPENAPI_CONTENT_TYPE: &str = "application/json; charset=utf-8";

#[derive(Clone)]
struct OpenApiDocument(Bytes);

#[derive(schemars::JsonSchema)]
struct ApiErrorResponse {
    #[schemars(rename = "Message")]
    _message: String,
}

pub(crate) fn documented_routes() -> Router<Arc<AppState>> {
    aide::generate::infer_responses(false);

    let mut document = base_document();
    let router = ApiRouter::<Arc<AppState>>::new()
        .api_route(
            "/health",
            get_with(super::health, |operation| {
                operation
                    .id("GetHealth")
                    .summary("Gets server health.")
                    .response_with::<200, String, _>(|response| {
                        plain_text_response(response, "The server and database are healthy.")
                    })
                    .response_with::<503, String, _>(|response| {
                        plain_text_response(response, "The server or database is unavailable.")
                    })
            }),
        )
        .api_route(
            "/System/Info/Public",
            get_with(public_system_info, |operation| {
                operation
                    .id("GetPublicSystemInfo")
                    .summary("Gets public information about the server.")
                    .response_with::<200, Json<PublicSystemInfo>, _>(|response| {
                        response.description("Public server information was retrieved.")
                    })
                    .response_with::<500, Json<ApiErrorResponse>, _>(|response| {
                        response.description("Server configuration could not be read.")
                    })
            }),
        )
        .api_route(
            "/System/Ping",
            get_with(super::ping, |operation| {
                operation
                    .id("GetPingSystem")
                    .summary("Pings the system.")
                    .response_with::<200, String, _>(|response| {
                        plain_text_response(response, "The server name was retrieved.")
                    })
            })
            .post_with(super::ping, |operation| {
                operation
                    .id("PostPingSystem")
                    .summary("Pings the system.")
                    .response_with::<200, String, _>(|response| {
                        plain_text_response(response, "The server name was retrieved.")
                    })
            }),
        )
        .api_route(
            "/api-docs/openapi.json",
            get_with(serve_document, |operation| {
                operation
                    .id("GetOpenApiSpec")
                    .summary("Gets the OpenAPI specification.")
                    .response_with::<200, Json<Value>, _>(|response| {
                        openapi_document_response(response)
                    })
            }),
        )
        .finish_api(&mut document);

    let document = OpenApiDocument(Bytes::from(
        serde_json::to_vec(&document).expect("the generated OpenAPI document must serialize"),
    ));

    router.layer(Extension(document))
}

fn base_document() -> OpenApi {
    let version = env!("CARGO_PKG_VERSION").to_owned();
    let mut info = Info {
        title: "Jellyfin API".to_owned(),
        version: version.clone(),
        ..Info::default()
    };
    info.extensions
        .insert("x-jellyfin-version".to_owned(), json!(version));

    let mut components = Components::default();
    components.security_schemes.insert(
        CUSTOM_AUTHENTICATION.to_owned(),
        ReferenceOr::Item(SecurityScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "Authorization".to_owned(),
            description: Some("API key header parameter".to_owned()),
            extensions: IndexMap::default(),
        }),
    );

    let mut document = OpenApi {
        info,
        components: Some(components),
        ..OpenApi::default()
    };
    document
        .extensions
        .insert("x-jellyfin-partial".to_owned(), json!(true));
    document
}

async fn public_system_info(state: State<Arc<AppState>>) -> Response {
    super::public_system_info(state).await.into_response()
}

async fn serve_document(Extension(document): Extension<OpenApiDocument>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static(OPENAPI_CONTENT_TYPE),
        )
        .body(Body::from(document.0))
        .expect("the static OpenAPI response must be valid")
}

fn plain_text_response<'response>(
    mut response: TransformResponse<'response, String>,
    description: &str,
) -> TransformResponse<'response, String> {
    response
        .inner()
        .content
        .get_mut("text/plain; charset=utf-8")
        .expect("Aide String responses must contain plain text")
        .schema = Some(SchemaObject {
        json_schema: json_schema!({ "type": "string" }),
        external_docs: None,
        example: None,
    });
    response.description(description)
}

fn openapi_document_response(
    mut response: TransformResponse<'_, Value>,
) -> TransformResponse<'_, Value> {
    response
        .inner()
        .content
        .get_mut("application/json")
        .expect("Aide JSON responses must contain application/json")
        .schema = Some(SchemaObject {
        json_schema: json_schema!({ "type": "object" }),
        external_docs: None,
        example: None,
    });
    response.description("The OpenAPI 3.1 document was retrieved.")
}
