//! Emby's administrator-only bulk metadata reset operation.
//!
//! The generated Java and Swift clients bind one required comma-separated
//! `ItemIds` query string and decode an empty HTTP 200 response. Keep this
//! adapter protocol-local: Jellyfin's unprefixed item-refresh contract remains
//! unchanged.

use std::{fmt, sync::Arc};

use axum::{
    Router,
    extract::{OriginalUri, Query, State, rejection::QueryRejection},
    http::HeaderMap,
    response::Response,
    routing::post,
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, de};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Items/Metadata/Reset", post(reset_metadata))
        .route("/items/metadata/reset", post(reset_metadata))
}

#[derive(Debug)]
struct MetadataResetQuery {
    item_ids: String,
}

impl<'de> Deserialize<'de> for MetadataResetQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = MetadataResetQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a query containing the required ItemIds value")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut item_ids = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("ItemIds") {
                        // ASP.NET's simple-value binding accepts repeated
                        // names and keeps the final value used by the action.
                        item_ids = Some(map.next_value::<String>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(MetadataResetQuery {
                    item_ids: item_ids.ok_or_else(|| de::Error::missing_field("ItemIds"))?,
                })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[allow(clippy::result_large_err)]
async fn reset_metadata(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<MetadataResetQuery>, QueryRejection>,
) -> Result<axum::http::StatusCode, Response> {
    let item_ids = query.ok().map(|Query(query)| query.item_ids);
    state
        .reset_emby_metadata_for_request(&headers, &uri, item_ids.as_deref())
        .await
}

#[cfg(test)]
mod tests {
    use super::MetadataResetQuery;

    #[test]
    fn item_ids_name_is_case_insensitive_and_last_duplicate_wins() {
        let parsed: MetadataResetQuery = serde_json::from_str(
            r#"{"ItemIds":"first","unknown":{"nested":true},"iTeMiDs":"second"}"#,
        )
        .expect("case-insensitive query");
        assert_eq!(parsed.item_ids, "second");

        assert!(serde_json::from_str::<MetadataResetQuery>(r#"{"other":"x"}"#).is_err());
    }
}
