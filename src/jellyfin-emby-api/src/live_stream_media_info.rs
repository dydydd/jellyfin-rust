//! Emby's legacy opened-live-stream media-information callback.
//!
//! Java and Swift generated clients decode no response body. The operation's
//! observable work is therefore the authenticated lookup/touch of a real
//! registry entry, including the official not-found behavior.

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
        .route("/LiveStreams/MediaInfo", post(media_info))
        .route("/livestreams/mediainfo", post(media_info))
}

#[derive(Debug)]
struct MediaInfoQuery {
    live_stream_id: String,
}

impl<'de> Deserialize<'de> for MediaInfoQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = MediaInfoQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a query containing the required LiveStreamId value")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut live_stream_id = None;
                while let Some(name) = map.next_key::<String>()? {
                    let value = map.next_value::<String>()?;
                    if name.eq_ignore_ascii_case("LiveStreamId") {
                        live_stream_id = Some(value);
                    }
                }
                Ok(MediaInfoQuery {
                    live_stream_id: live_stream_id
                        .ok_or_else(|| de::Error::missing_field("LiveStreamId"))?,
                })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[allow(clippy::result_large_err)]
async fn media_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<MediaInfoQuery>, QueryRejection>,
) -> Result<axum::http::StatusCode, Response> {
    let live_stream_id = query.ok().map(|Query(query)| query.live_stream_id);
    state
        .emby_live_stream_media_info_for_request(&headers, &uri, live_stream_id.as_deref())
        .await
}

#[cfg(test)]
mod tests {
    use super::MediaInfoQuery;

    #[test]
    fn query_name_is_case_insensitive_and_last_duplicate_wins() {
        let parsed: MediaInfoQuery =
            serde_json::from_str(r#"{"LiveStreamId":"first","other":"x","lIvEsTrEaMiD":"second"}"#)
                .expect("case-insensitive query");
        assert_eq!(parsed.live_stream_id, "second");

        assert!(serde_json::from_str::<MediaInfoQuery>(r#"{"other":"x"}"#).is_err());
    }
}
