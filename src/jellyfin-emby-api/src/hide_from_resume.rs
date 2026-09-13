//! Emby's reversible continue-watching suppression endpoint.
//!
//! The wire response remains the ordinary `UserItemDataDto`; the suppression
//! bit is internal and affects only queries that explicitly select resumable
//! candidates.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, Query, State, rejection::QueryRejection},
    http::HeaderMap,
    response::Response,
    routing::post,
};
use jellyfin_api::AppState;
use jellyfin_model::UserItemDataDto;
use serde::{Deserialize, Deserializer, de};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Users/{user_id}/Items/{item_id}/HideFromResume",
            post(set_hidden),
        )
        .route(
            "/users/{user_id}/items/{item_id}/hidefromresume",
            post(set_hidden),
        )
}

#[derive(Debug)]
struct HideQuery {
    hide: bool,
}

impl<'de> Deserialize<'de> for HideQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = HideQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a query containing the required boolean Hide value")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut hide = None;
                while let Some(name) = map.next_key::<String>()? {
                    let value = map.next_value::<String>()?;
                    if name.eq_ignore_ascii_case("Hide") {
                        hide = Some(
                            parse_bool(&value)
                                .ok_or_else(|| de::Error::custom("Hide must be a boolean"))?,
                        );
                    }
                }
                Ok(HideQuery {
                    hide: hide.ok_or_else(|| de::Error::missing_field("Hide"))?,
                })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("true") {
        Some(true)
    } else if value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

#[allow(clippy::result_large_err)]
async fn set_hidden(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(String, String)>,
    query: Result<Query<HideQuery>, QueryRejection>,
) -> Result<Json<UserItemDataDto>, Response> {
    let hide = query.ok().map(|Query(query)| query.hide);
    state
        .set_emby_hidden_from_resume_for_request(&headers, &uri, &user_id, &item_id, hide)
        .await
        .map(Json)
}
