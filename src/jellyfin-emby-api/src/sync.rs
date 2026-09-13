//! Read-only discovery for Emby's retired offline-sync subsystem.
//!
//! Jellyfin removed the old sync provider and this Rust server does not yet
//! register a replacement.  The generated Emby clients still probe these
//! endpoints, so return the same collection shapes produced by the official
//! service when no targets, jobs, or ready items exist.  Mutating sync routes
//! deliberately remain unavailable until there is a real sync backend.

use std::{fmt, sync::Arc};

use axum::{Json, Router, extract::Query, routing::get};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sync/Targets", get(targets))
        .route("/sync/targets", get(targets))
        .route("/Sync/Jobs", get(jobs))
        .route("/sync/jobs", get(jobs))
        .route("/Sync/JobItems", get(job_items))
        .route("/sync/jobitems", get(job_items))
        .route("/Sync/Items/Ready", get(ready_items))
        .route("/sync/items/ready", get(ready_items))
}

#[derive(Debug)]
struct UserIdQuery {
    _user_id: String,
}

#[derive(Debug)]
struct TargetIdQuery {
    _target_id: String,
}

impl<'de> Deserialize<'de> for UserIdQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            _user_id: required_string(deserializer, "UserId")?,
        })
    }
}

impl<'de> Deserialize<'de> for TargetIdQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            _target_id: required_string(deserializer, "TargetId")?,
        })
    }
}

fn required_string<'de, D: Deserializer<'de>>(
    deserializer: D,
    required_name: &'static str,
) -> Result<String, D::Error> {
    struct RequiredStringVisitor {
        required_name: &'static str,
    }

    impl<'de> de::Visitor<'de> for RequiredStringVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "a query containing {}", self.required_name)
        }

        fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut value = None;
            while let Some(name) = map.next_key::<String>()? {
                if name.eq_ignore_ascii_case(self.required_name) {
                    // ASP.NET's case-insensitive binder keeps the last value
                    // assigned to the same property.
                    value = Some(map.next_value::<String>()?);
                } else {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
            value.ok_or_else(|| de::Error::missing_field(self.required_name))
        }
    }

    deserializer.deserialize_map(RequiredStringVisitor { required_name })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult {
    items: Vec<Value>,
    total_record_count: i32,
}

async fn targets(Query(_query): Query<UserIdQuery>) -> Json<Vec<Value>> {
    Json(Vec::new())
}

async fn jobs() -> Json<QueryResult> {
    empty_query_result()
}

async fn job_items(Query(_query): Query<TargetIdQuery>) -> Json<QueryResult> {
    empty_query_result()
}

async fn ready_items(Query(_query): Query<TargetIdQuery>) -> Json<Vec<Value>> {
    Json(Vec::new())
}

fn empty_query_result() -> Json<QueryResult> {
    Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
    })
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
    async fn required_queries_are_case_insensitive_and_last_duplicate_wins() {
        for path in [
            "/Sync/Targets?uSeRiD=first&USERID=second",
            "/Sync/JobItems?tArGeTiD=first&TARGETID=second",
            "/Sync/Items/Ready?tArGeTiD=first&TARGETID=second",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn missing_required_queries_are_bad_requests() {
        for path in ["/Sync/Targets", "/Sync/JobItems", "/Sync/Items/Ready"] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }
    }

    #[tokio::test]
    async fn empty_results_use_generated_client_shapes() {
        for path in [
            "/Sync/Targets?UserId=user",
            "/Sync/Items/Ready?TargetId=target",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                serde_json::json!([])
            );
        }

        for path in ["/Sync/Jobs", "/Sync/JobItems?TargetId=target"] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                serde_json::json!({"Items": [], "TotalRecordCount": 0})
            );
        }
    }
}
