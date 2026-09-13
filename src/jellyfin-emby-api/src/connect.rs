//! Protocol-local compatibility for the retired Emby Connect service.
//!
//! Connect predates Jellyfin QuickConnect and depended on Emby's hosted
//! `connect.emby.media` identity provider. This server has no Connect provider
//! or persisted Connect identity mapping, so discovery is truthfully empty,
//! exchange never fabricates a token, and link creation fails explicitly
//! without mutating the target user. Unlink remains idempotent like the
//! historical official implementation.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Connect/Pending", get(pending))
        .route("/Connect/Exchange", get(exchange))
        .route("/Users/{user_id}/Connect/Link", post(link).delete(unlink))
        .route("/Users/{user_id}/Connect/Link/Delete", post(unlink))
}

#[allow(clippy::result_large_err)]
async fn pending(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(Vec::new()))
}

#[allow(clippy::result_large_err)]
async fn exchange(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, Response> {
    state.require_emby_user(&headers, &uri).await?;
    required_query_value(&uri, "ConnectUserId", false)?;

    // There is no persisted ConnectUserId mapping or hosted provider from
    // which one could be refreshed. A fabricated local id/token would turn a
    // retired login mechanism into an authentication bypass.
    Err(StatusCode::NOT_FOUND.into_response())
}

#[allow(clippy::result_large_err)]
async fn link(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, Response> {
    let user_id = path.map_err(IntoResponse::into_response)?.0;
    state
        .require_emby_administrator_user(&headers, &uri, user_id)
        .await?;
    required_query_value(&uri, "ConnectUsername", true)?;

    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"Message": "Emby Connect provider is unavailable"})),
    )
        .into_response())
}

#[allow(clippy::result_large_err)]
async fn unlink(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, Response> {
    let user_id = path.map_err(IntoResponse::into_response)?.0;
    state
        .require_emby_administrator_user(&headers, &uri, user_id)
        .await?;
    Ok(StatusCode::OK)
}

fn required_query_value<'a>(
    uri: &'a Uri,
    required_name: &str,
    reject_blank: bool,
) -> Result<String, Response> {
    let mut matched = None;
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case(required_name) {
            matched = Some(value.into_owned());
        }
    }
    let value = matched.ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
    if reject_blank && value.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::required_query_value;

    #[test]
    fn connect_queries_are_case_insensitive_and_last_duplicate_wins() {
        let uri = Uri::from_static(
            "/emby/Connect/Exchange?CONNECTUSERID=first&other=x&connectuserid=second",
        );
        assert_eq!(
            required_query_value(&uri, "ConnectUserId", false).expect("required query"),
            "second"
        );

        let uri = Uri::from_static(
            "/emby/Users/id/Connect/Link?ConnectUsername=first&connectusername=%20%20",
        );
        assert!(required_query_value(&uri, "ConnectUsername", true).is_err());
        assert!(required_query_value(&uri, "missing", false).is_err());
    }
}
