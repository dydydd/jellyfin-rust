use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PluginInfo {
    name: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration_file_name: Option<String>,
    description: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_tag: Option<String>,
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Plugins", get(list))
        .route("/plugins", get(list))
        .route("/Plugins/{plugin_id}/Configuration", get(configuration))
        .route("/plugins/{plugin_id}/configuration", get(configuration))
        .route("/Plugins/{plugin_id}/Thumb", get(thumb))
        .route("/plugins/{plugin_id}/thumb", get(thumb))
}

async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<PluginInfo>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(
        state
            .emby_plugins()
            .into_iter()
            .map(|p| PluginInfo {
                image_tag: p.has_image.then(|| format!("/emby/Plugins/{}/Thumb", p.id)),
                name: p.name,
                version: p.version,
                configuration_file_name: p.configuration_file_name,
                description: p.description,
                id: p.id.simple().to_string(),
            })
            .collect(),
    ))
}

async fn configuration(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(plugin_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    state
        .emby_plugin_configuration(plugin_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
        .ok_or(StatusCode::NOT_FOUND.into_response())
        .map(Json)
}

async fn thumb(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(plugin_id): Path<Uuid>,
) -> Result<Response, Response> {
    state.require_emby_user(&headers, &uri).await?;
    let Some(image) = state.emby_plugin_image(plugin_id) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let request = axum::http::Request::builder()
        .method("GET")
        .body(Body::empty())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let response = ServeFile::new(PathBuf::from(image.path))
        .oneshot(request)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?;
    let mut response = response.map(Body::new);
    response.headers_mut().remove(header::ACCEPT_RANGES);
    if response.status().is_success() {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            image
                .mime_type
                .parse()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?,
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_static("attachment"),
        );
    }
    Ok(response)
}
