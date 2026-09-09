use axum::{
    Json, Router,
    extract::{OriginalUri, Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct QueryParams {
    #[serde(alias = "AssemblyGuid", alias = "assemblyguid")]
    assembly_guid: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageInfo {
    id: String,
    name: String,
    short_description: String,
    overview: String,
    owner: String,
    category: String,
    guid: String,
    versions: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb_image: Option<String>,
}

impl From<&jellyfin_model::PackageInfo> for PackageInfo {
    fn from(p: &jellyfin_model::PackageInfo) -> Self {
        Self {
            id: p.id.simple().to_string(),
            guid: p.id.simple().to_string(),
            name: p.name.clone(),
            short_description: p.description.clone(),
            overview: p.overview.clone(),
            owner: p.owner.clone(),
            category: p.category.clone(),
            versions: p.versions.clone(),
            thumb_image: p.image_url.clone(),
        }
    }
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Packages", get(list))
        .route("/packages", get(list))
        .route("/Packages/{name}", get(get_package))
        .route("/packages/{name}", get(get_package))
}

async fn list(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<PackageInfo>>, Response> {
    state.require_emby_user(&headers, &uri).await?;
    Ok(Json(
        state
            .emby_packages()
            .iter()
            .map(|p| PackageInfo::from(p.as_ref()))
            .collect(),
    ))
}

async fn get_package(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<QueryParams>,
) -> Result<Json<PackageInfo>, Response> {
    state.require_emby_user(&headers, &uri).await?;
    let package = state
        .emby_package(&name, query.assembly_guid)
        .map_err(|_| axum::http::StatusCode::NOT_FOUND.into_response())?;
    Ok(Json(PackageInfo::from(package.as_ref())))
}
