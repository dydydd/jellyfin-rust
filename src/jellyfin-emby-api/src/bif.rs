//! Emby's Roku BIF compatibility route.
//!
//! Jellyfin trickplay stores composite JPEG sprite tiles and an HLS image
//! playlist. A BIF file instead contains a binary timestamp index followed by
//! individually encoded JPEG frames. The two formats cannot be mapped without
//! decoding and cropping the stored sprites, so the adapter reports the BIF as
//! unavailable instead of sending a tile or the source video under the BIF
//! filename.

use std::{fmt, sync::Arc};

use axum::{
    Router,
    extract::{Path, Query},
    http::StatusCode,
    routing::get,
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, de};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Videos/{item_id}/index.bif", get(index))
        .route("/videos/{item_id}/index.bif", get(index))
}

#[derive(Debug)]
struct BifQuery;

impl<'de> Deserialize<'de> for BifQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BifQueryVisitor;

        impl<'de> de::Visitor<'de> for BifQueryVisitor {
            type Value = BifQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby BIF query containing Width")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut has_width = false;
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("Width") {
                        // ASP.NET binds the value case-insensitively as a
                        // signed Int32.
                        let _: i32 = map.next_value()?;
                        has_width = true;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                if !has_width {
                    return Err(de::Error::missing_field("Width"));
                }
                Ok(BifQuery)
            }
        }

        deserializer.deserialize_map(BifQueryVisitor)
    }
}

async fn index(Path(_item_id): Path<String>, Query(BifQuery): Query<BifQuery>) -> StatusCode {
    // The enclosing Emby protocol middleware has already applied ordinary
    // authenticated-user access before query/path binding. Width is still
    // bound here so missing, malformed, and out-of-range SDK values remain a
    // 400 rather than being hidden behind the unavailable-resource response.
    StatusCode::NOT_FOUND
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, header},
    };
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    fn app() -> Router {
        routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )))
    }

    #[tokio::test]
    async fn bif_unavailable_response_does_not_claim_range_support() {
        for path in [
            "/Videos/not-a-guid/index.bif?Width=320",
            "/videos/not-a-guid/index.bif?width=-1",
            "/Videos/not-a-guid/index.bif?wIdTh=0",
        ] {
            let response = app()
                .oneshot(
                    Request::get(path)
                        .header(header::RANGE, "bytes=0-99")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(response.headers().get(header::ACCEPT_RANGES).is_none());
            assert!(response.headers().get(header::CONTENT_RANGE).is_none());
            assert!(
                to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn width_uses_required_signed_int32_binding() {
        for path in [
            "/Videos/id/index.bif",
            "/Videos/id/index.bif?Width=not-a-number",
            "/Videos/id/index.bif?Width=2147483648",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
    }
}
