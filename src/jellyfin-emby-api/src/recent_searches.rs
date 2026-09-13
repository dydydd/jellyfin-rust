//! Emby's per-user recently-searched state routes.
//!
//! The generated `ReportItemsSearched` wire DTO contains only a nullable
//! `WasSearched` boolean. It carries neither a search term nor an item id, so
//! this adapter persists exactly that per-user state instead of fabricating a
//! search-history representation that clients never sent.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, State, rejection::JsonRejection},
    http::HeaderMap,
    response::Response,
    routing::{delete, post},
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, de};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Users/{user_id}/RecentlySearched",
            delete(clear_recently_searched),
        )
        .route(
            "/users/{user_id}/recentlysearched",
            delete(clear_recently_searched),
        )
        .route(
            "/Users/{user_id}/RecentlySearched/Delete",
            post(clear_recently_searched),
        )
        .route(
            "/users/{user_id}/recentlysearched/delete",
            post(clear_recently_searched),
        )
        .route(
            "/Users/{user_id}/SearchedItems/",
            post(report_items_searched),
        )
        .route(
            "/users/{user_id}/searcheditems/",
            post(report_items_searched),
        )
        // Some HTTP clients normalize away a terminal slash. Keep that
        // transport normalization equivalent to the generated literal route.
        .route(
            "/Users/{user_id}/SearchedItems",
            post(report_items_searched),
        )
        .route(
            "/users/{user_id}/searcheditems",
            post(report_items_searched),
        )
}

#[derive(Debug, Default)]
struct ReportItemsSearched {
    was_searched: Option<bool>,
}

impl<'de> Deserialize<'de> for ReportItemsSearched {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = ReportItemsSearched;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object containing an optional boolean WasSearched value")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut was_searched = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("WasSearched") {
                        was_searched = map.next_value::<Option<bool>>()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(ReportItemsSearched { was_searched })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[allow(clippy::result_large_err)]
async fn clear_recently_searched(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Result<axum::http::StatusCode, Response> {
    state
        .clear_emby_recently_searched_for_request(&headers, &uri, &user_id)
        .await
}

#[allow(clippy::result_large_err)]
async fn report_items_searched(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    body: Result<Json<ReportItemsSearched>, JsonRejection>,
) -> Result<axum::http::StatusCode, Response> {
    let reported = body.ok().map(|Json(body)| body.was_searched);
    state
        .report_emby_items_searched_for_request(&headers, &uri, &user_id, reported)
        .await
}

#[cfg(test)]
mod tests {
    use super::ReportItemsSearched;

    #[test]
    fn report_body_is_case_insensitive_last_duplicate_wins_and_ignores_unknown_fields() {
        let parsed: ReportItemsSearched = serde_json::from_str(
            r#"{"WasSearched":true,"Unknown":{"nested":1},"wAsSeArChEd":false}"#,
        )
        .expect("case-insensitive report body");
        assert_eq!(parsed.was_searched, Some(false));

        let parsed: ReportItemsSearched =
            serde_json::from_str(r#"{"WASSEARCHED":true}"#).expect("uppercase field");
        assert_eq!(parsed.was_searched, Some(true));
    }

    #[test]
    fn report_body_accepts_nullable_or_omitted_flag_and_rejects_wrong_shapes() {
        for body in [r#"{}"#, r#"{"WasSearched":null}"#] {
            let parsed: ReportItemsSearched = serde_json::from_str(body).expect("nullable field");
            assert_eq!(parsed.was_searched, None);
        }
        for body in [
            r#"[]"#,
            r#"true"#,
            r#"{"WasSearched":"true"}"#,
            r#"{"WasSearched":1}"#,
        ] {
            assert!(serde_json::from_str::<ReportItemsSearched>(body).is_err());
        }
    }
}
