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
        .route("/Sync/Options", get(options))
        .route("/sync/options", get(options))
}

#[derive(Debug)]
struct UserIdQuery {
    _user_id: String,
}

#[derive(Debug)]
struct TargetIdQuery {
    _target_id: String,
}

#[derive(Debug, PartialEq, Eq)]
struct SyncOptionsQuery {
    user_id: String,
    item_ids: Option<String>,
    parent_id: Option<String>,
    target_id: Option<String>,
    category: Option<SyncCategory>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncCategory {
    Latest,
    NextUp,
    Resume,
}

impl SyncCategory {
    fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("Latest") || value == "1" {
            Some(Self::Latest)
        } else if value.eq_ignore_ascii_case("NextUp") || value == "2" {
            Some(Self::NextUp)
        } else if value.eq_ignore_ascii_case("Resume") || value == "3" {
            Some(Self::Resume)
        } else {
            None
        }
    }
}

impl<'de> Deserialize<'de> for SyncCategory {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| {
            de::Error::unknown_variant(&value, &["Latest", "NextUp", "Resume", "1", "2", "3"])
        })
    }
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

impl<'de> Deserialize<'de> for SyncOptionsQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = SyncOptionsQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby Sync/Options query")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut user_id = None;
                let mut item_ids = None;
                let mut parent_id = None;
                let mut target_id = None;
                let mut category = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("UserId") {
                        user_id = Some(map.next_value::<String>()?);
                    } else if name.eq_ignore_ascii_case("ItemIds") {
                        item_ids = Some(map.next_value::<String>()?);
                    } else if name.eq_ignore_ascii_case("ParentId") {
                        parent_id = Some(map.next_value::<String>()?);
                    } else if name.eq_ignore_ascii_case("TargetId") {
                        target_id = Some(map.next_value::<String>()?);
                    } else if name.eq_ignore_ascii_case("Category") {
                        // Retain the raw final duplicate before parsing so an
                        // earlier invalid value cannot defeat ASP.NET's
                        // last-assignment-wins property binding.
                        category = Some(map.next_value::<String>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(SyncOptionsQuery {
                    user_id: user_id.ok_or_else(|| de::Error::missing_field("UserId"))?,
                    item_ids,
                    parent_id,
                    target_id,
                    category: category
                        .map(|value| {
                            SyncCategory::parse(&value).ok_or_else(|| {
                                de::Error::unknown_variant(
                                    &value,
                                    &["Latest", "NextUp", "Resume", "1", "2", "3"],
                                )
                            })
                        })
                        .transpose()?,
                })
            }
        }

        deserializer.deserialize_map(Visitor)
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SyncDialogOptions {
    targets: Vec<Value>,
    options: Vec<Value>,
    quality_options: Vec<Value>,
    profile_options: Vec<Value>,
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

// With no registered legacy sync provider there are no honest targets,
// profiles, quality choices, or job options to advertise. Explicit empty
// arrays retain the generated clients' collection shape without claiming an
// unavailable offline-sync capability.
async fn options(Query(_query): Query<SyncOptionsQuery>) -> Json<SyncDialogOptions> {
    Json(SyncDialogOptions {
        targets: Vec::new(),
        options: Vec::new(),
        quality_options: Vec::new(),
        profile_options: Vec::new(),
    })
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

        let response = app()
            .oneshot(
                Request::get("/Sync/Options?UserId=user")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            serde_json::json!({
                "Targets": [],
                "Options": [],
                "QualityOptions": [],
                "ProfileOptions": []
            })
        );
    }

    #[test]
    fn sync_options_query_binds_all_fields_case_insensitively_and_last_wins() {
        let query: SyncOptionsQuery = serde_json::from_str(
            r#"{"UserId":"first","userid":"second","ItemIds":"1,2","itemids":"3","ParentId":"parent","parentid":"other-parent","TargetId":"target","targetid":"other-target","Category":"Latest","category":"resume","Ignored":"value"}"#,
        )
        .expect("Sync/Options fields");
        assert_eq!(
            query,
            SyncOptionsQuery {
                user_id: "second".to_owned(),
                item_ids: Some("3".to_owned()),
                parent_id: Some("other-parent".to_owned()),
                target_id: Some("other-target".to_owned()),
                category: Some(SyncCategory::Resume),
            }
        );
        assert!(
            serde_json::from_str::<SyncOptionsQuery>(r#"{"UserId":"user","Category":"2"}"#).is_ok()
        );
        assert!(
            serde_json::from_str::<SyncOptionsQuery>(r#"{"UserId":"user","Category":"unknown"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<SyncOptionsQuery>(r#"{"Category":"Latest"}"#).is_err());
    }
}
