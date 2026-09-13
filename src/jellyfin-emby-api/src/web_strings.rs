//! Emby's public plugin-localization discovery aliases.
//!
//! The generated Swagger incorrectly marks these routes as authenticated and
//! omits their query surface. Emby 4.10 marks both request DTOs
//! `Unauthenticated`; `/web/strings` binds `PluginId` and `Locale`, while
//! `/web/stringset` binds `PluginId`.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{OriginalUri, State},
    http::{StatusCode, header},
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/web/strings", get(strings))
        .route("/web/stringset", get(string_set))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct StringsQuery {
    plugin_id: Option<String>,
    locale: Option<String>,
}

async fn strings(State(state): State<Arc<AppState>>, OriginalUri(uri): OriginalUri) -> Response {
    let query = parse_query(uri.query(), true);
    match resolve_plugin(&state, query.plugin_id.as_deref()) {
        // PluginInfo only proves installation. Emby's action casts the
        // matching plugin to IHasTranslations and its localization manager
        // dereferences the missing provider, so metadata-only plugins fail.
        // Do not invent a successful empty dictionary until the Rust runtime
        // has a real translation-provider registry.
        Ok(()) => plain_text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Object reference not set to an instance of an object.",
        ),
        Err(response) => response,
    }
}

async fn string_set(State(state): State<Arc<AppState>>, OriginalUri(uri): OriginalUri) -> Response {
    let query = parse_query(uri.query(), false);
    match resolve_plugin(&state, query.plugin_id.as_deref()) {
        Ok(()) => json_response("[]"),
        Err(response) => response,
    }
}

fn parse_query(raw_query: Option<&str>, include_locale: bool) -> StringsQuery {
    let mut query = StringsQuery::default();
    for (name, value) in form_urlencoded::parse(raw_query.unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("PluginId") {
            query.plugin_id = Some(value.into_owned());
        } else if include_locale && name.eq_ignore_ascii_case("Locale") {
            query.locale = Some(value.into_owned());
        }
    }
    query
}

fn resolve_plugin(state: &AppState, plugin_id: Option<&str>) -> Result<(), Response> {
    let plugin_id = plugin_id.unwrap_or("00000000-0000-0000-0000-000000000000");
    let plugin_id = Uuid::parse_str(plugin_id).map_err(|_| {
        plain_text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unrecognized Guid format.",
        )
    })?;
    if state
        .emby_plugins()
        .iter()
        .any(|plugin| plugin.id == plugin_id)
    {
        Ok(())
    } else {
        Err(plain_text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Sequence contains no matching element",
        ))
    }
}

fn json_response(body: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(body))
        .expect("static web strings response headers are valid")
}

fn plain_text(status: StatusCode, body: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(body))
        .expect("static web strings response headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_binding_is_case_insensitive_last_wins_and_locale_is_route_specific() {
        let parsed = parse_query(
            Some("PluginId=first&LOCALE=de-DE&pLuGiNiD=second&locale=en-US"),
            true,
        );
        assert_eq!(
            parsed,
            StringsQuery {
                plugin_id: Some("second".to_owned()),
                locale: Some("en-US".to_owned()),
            }
        );

        let set = parse_query(Some("PLUGINID=id&Locale=en-US"), false);
        assert_eq!(set.plugin_id.as_deref(), Some("id"));
        assert_eq!(set.locale, None);
    }
}
