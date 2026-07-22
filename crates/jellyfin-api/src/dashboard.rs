use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, Request, header},
    response::{IntoResponse, Response},
};
use jellyfin_controller::DashboardPage;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

const JSON_UTF8: HeaderValue = HeaderValue::from_static("application/json; charset=utf-8");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ConfigurationPageQuery {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ConfigurationPagesQuery {
    #[serde(rename = "enableInMainMenu", alias = "EnableInMainMenu")]
    enable_in_main_menu: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ConfigurationPageInfo {
    name: String,
    enable_in_main_menu: bool,
    menu_section: Option<String>,
    menu_icon: Option<String>,
    display_name: Option<String>,
    plugin_id: Option<Uuid>,
}

pub(crate) async fn configuration_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ConfigurationPageQuery>,
) -> Result<Response, ApiError> {
    let name = query
        .name
        .as_deref()
        .ok_or(jellyfin_controller::DashboardError::NotFound)?;
    let path = state.dashboard.resolve_page(name).await?;
    let mut request = Request::builder()
        .method("GET")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    for name in [header::RANGE, header::IF_RANGE] {
        if let Some(value) = headers.get(&name) {
            request.headers_mut().insert(name, value.clone());
        }
    }
    let response = match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    Ok(response.map(Body::new))
}

pub(crate) async fn configuration_pages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ConfigurationPagesQuery>,
) -> Result<Response, ApiError> {
    require_administrator(&state, &headers).await?;
    let pages = state
        .dashboard
        .configuration_pages(query.enable_in_main_menu)
        .into_iter()
        .map(ConfigurationPageInfo::from)
        .collect::<Vec<_>>();
    let mut response = Json(pages).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, JSON_UTF8);
    Ok(response)
}

impl From<DashboardPage> for ConfigurationPageInfo {
    fn from(page: DashboardPage) -> Self {
        Self {
            name: page.name,
            enable_in_main_menu: page.enable_in_main_menu,
            menu_section: page.menu_section,
            menu_icon: page.menu_icon,
            display_name: page.display_name,
            plugin_id: page.plugin_id,
        }
    }
}

async fn require_administrator(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let authenticated = authentication::authenticated_session(state, headers).await?;
    if authenticated.user.is_administrator {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}
