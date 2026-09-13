//! Protocol-local endpoints for Emby's retired Generic UI controller system.
//!
//! Generic UI pages are registered by Emby plugins. The Rust server does not
//! load those page controllers, so a syntactically valid request has no target
//! page. Emby 4.10 reports that condition as a plain-text HTTP 500 rather than
//! fabricating an empty `UIViewInfo`.

use std::{fmt, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, de};

const MAX_COMMAND_BYTES: usize = 1024 * 1024;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/UI/View", get(view))
        .route("/ui/view", get(view))
        .route("/UI/Command", post(command))
        .route("/ui/command", post(command))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ViewQuery {
    page_id: Option<String>,
    client_locale: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RunUiCommand {
    page_id: Option<String>,
    command_id: Option<String>,
    data: Option<String>,
    item_id: Option<String>,
    client_locale: Option<String>,
}

impl<'de> Deserialize<'de> for RunUiCommand {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = RunUiCommand;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby RunUICommand object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut command = RunUiCommand::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("PageId") {
                        command.page_id = map.next_value::<Option<String>>()?;
                    } else if name.eq_ignore_ascii_case("CommandId") {
                        command.command_id = map.next_value::<Option<String>>()?;
                    } else if name.eq_ignore_ascii_case("Data") {
                        command.data = map.next_value::<Option<String>>()?;
                    } else if name.eq_ignore_ascii_case("ItemId") {
                        command.item_id = map.next_value::<Option<String>>()?;
                    } else if name.eq_ignore_ascii_case("ClientLocale") {
                        command.client_locale = map.next_value::<Option<String>>()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(command)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

async fn view(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = state.require_emby_administrator(&headers, &uri).await {
        return response;
    }

    let query = parse_view_query(uri.query());
    let Some(page_id) = query.page_id else {
        return null_page_id();
    };
    unavailable_page(&page_id)
}

async fn command(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if let Err(response) = state.require_emby_administrator(&headers, &uri).await {
        return response;
    }

    let Ok(body) = to_bytes(request.into_body(), MAX_COMMAND_BYTES).await else {
        return bad_command_body();
    };
    let Ok(command) = serde_json::from_slice::<RunUiCommand>(&body) else {
        return bad_command_body();
    };
    let Some(page_id) = command.page_id else {
        return null_page_id();
    };
    unavailable_page(&page_id)
}

fn parse_view_query(raw_query: Option<&str>) -> ViewQuery {
    let mut query = ViewQuery::default();
    for (name, value) in form_urlencoded::parse(raw_query.unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("PageId") {
            query.page_id = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("ClientLocale") {
            query.client_locale = Some(value.into_owned());
        }
    }
    query
}

fn null_page_id() -> Response {
    plain_text(
        StatusCode::BAD_REQUEST,
        "Value cannot be null. (Parameter 'key')",
    )
}

fn bad_command_body() -> Response {
    plain_text(StatusCode::BAD_REQUEST, "Invalid request body")
}

fn unavailable_page(page_id: &str) -> Response {
    plain_text(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Unable to find the specified target page (ID: {page_id})"),
    )
}

fn plain_text(status: StatusCode, body: impl Into<Body>) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(body.into())
        .expect("static Generic UI response headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_query_is_case_insensitive_and_last_duplicate_wins() {
        let parsed = parse_view_query(Some(
            "PageId=first&CLIENTLOCALE=de-DE&pAgEiD=second&clientlocale=en-US",
        ));
        assert_eq!(
            parsed,
            ViewQuery {
                page_id: Some("second".to_owned()),
                client_locale: Some("en-US".to_owned()),
            }
        );
    }

    #[test]
    fn command_body_binds_every_property_case_insensitively_and_last_wins() {
        let parsed: RunUiCommand = serde_json::from_str(
            r#"{
                "PageId":"first",
                "CommandId":"save",
                "Data":"payload",
                "ItemId":"item",
                "ClientLocale":"de-DE",
                "unknown":{"nested":true},
                "pAgEiD":"second",
                "cLiEnTlOcAlE":"en-US"
            }"#,
        )
        .expect("valid RunUICommand");
        assert_eq!(parsed.page_id.as_deref(), Some("second"));
        assert_eq!(parsed.command_id.as_deref(), Some("save"));
        assert_eq!(parsed.data.as_deref(), Some("payload"));
        assert_eq!(parsed.item_id.as_deref(), Some("item"));
        assert_eq!(parsed.client_locale.as_deref(), Some("en-US"));
    }
}
