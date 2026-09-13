//! Emby package-update discovery.
//!
//! The Rust server does not expose an operational package update provider, so
//! the truthful result is an empty generated-client collection. Emby still
//! requires the `PackageType` query member and administrator authentication;
//! the parent protocol middleware enforces authentication before this binder.

use std::{fmt, sync::Arc};

use axum::{Json, Router, extract::Query, routing::get};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, de};
use serde_json::Value;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Packages/Updates", get(updates))
        .route("/packages/updates", get(updates))
}

#[derive(Debug)]
struct PackageTypeQuery {
    _package_type: String,
}

impl<'de> Deserialize<'de> for PackageTypeQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PackageTypeVisitor;

        impl<'de> de::Visitor<'de> for PackageTypeVisitor {
            type Value = PackageTypeQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a query containing PackageType")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut package_type = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("PackageType") {
                        // ASP.NET binds property names case-insensitively and
                        // retains the final assignment for repeated keys.
                        package_type = Some(map.next_value::<String>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                package_type
                    .map(|_package_type| PackageTypeQuery { _package_type })
                    .ok_or_else(|| de::Error::missing_field("PackageType"))
            }
        }

        deserializer.deserialize_map(PackageTypeVisitor)
    }
}

async fn updates(Query(_query): Query<PackageTypeQuery>) -> Json<Vec<Value>> {
    Json(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
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
    async fn package_type_is_required_case_insensitive_and_last_wins() {
        for path in [
            "/Packages/Updates?PackageType=System",
            "/Packages/Updates?pAcKaGeTyPe=System&PACKAGETYPE=UserInstalled",
            "/packages/updates?packagetype=System",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                serde_json::from_slice::<Value>(
                    &to_bytes(response.into_body(), 1024).await.unwrap()
                )
                .unwrap(),
                serde_json::json!([]),
                "{path}",
            );
        }

        let response = app()
            .oneshot(
                Request::get("/Packages/Updates")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
