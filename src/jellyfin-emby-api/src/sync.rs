//! Compatibility surface for Emby's retired offline-sync subsystem.
//!
//! Jellyfin removed the old sync provider and this Rust server does not yet
//! register a replacement.  The generated Emby clients still probe these
//! endpoints, so return the same collection shapes produced by the official
//! service when no targets, jobs, or ready items exist. Object and file
//! lookups and mutations return not found because there is no provider-owned
//! record to resolve. `Sync/Data` can honestly report an empty removal set.

use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{
        Path, Query,
        rejection::{JsonRejection, QueryRejection},
    },
    http::StatusCode,
    routing::{delete, get, post},
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Sync/Targets", get(targets))
        .route("/sync/targets", get(targets))
        .route("/Sync/Jobs", get(jobs).post(create_job))
        .route("/sync/jobs", get(jobs).post(create_job))
        .route(
            "/Sync/Jobs/{id}",
            get(sync_job).post(update_job).delete(unavailable_id),
        )
        .route(
            "/sync/jobs/{id}",
            get(sync_job).post(update_job).delete(unavailable_id),
        )
        .route("/Sync/JobItems", get(job_items))
        .route("/sync/jobitems", get(job_items))
        .route("/Sync/JobItems/{id}", delete(unavailable_id))
        .route("/sync/jobitems/{id}", delete(unavailable_id))
        .route("/Sync/JobItems/{id}/File", get(job_item_file))
        .route("/sync/jobitems/{id}/file", get(job_item_file))
        .route(
            "/Sync/JobItems/{id}/AdditionalFiles",
            get(job_item_additional_file),
        )
        .route(
            "/sync/jobitems/{id}/additionalfiles",
            get(job_item_additional_file),
        )
        .route("/Sync/JobItems/{id}/Transferred", post(unavailable_id))
        .route("/sync/jobitems/{id}/transferred", post(unavailable_id))
        .route("/Sync/JobItems/{id}/Enable", post(unavailable_id))
        .route("/sync/jobitems/{id}/enable", post(unavailable_id))
        .route("/Sync/JobItems/{id}/Delete", post(unavailable_id))
        .route("/sync/jobitems/{id}/delete", post(unavailable_id))
        .route("/Sync/JobItems/{id}/MarkForRemoval", post(unavailable_id))
        .route("/sync/jobitems/{id}/markforremoval", post(unavailable_id))
        .route("/Sync/JobItems/{id}/UnmarkForRemoval", post(unavailable_id))
        .route("/sync/jobitems/{id}/unmarkforremoval", post(unavailable_id))
        .route("/Sync/Jobs/{id}/Delete", post(unavailable_id))
        .route("/sync/jobs/{id}/delete", post(unavailable_id))
        .route("/Sync/OfflineActions", post(offline_actions))
        .route("/sync/offlineactions", post(offline_actions))
        .route("/Sync/Data", post(sync_data))
        .route("/sync/data", post(sync_data))
        .route("/Sync/Items/Cancel", post(cancel_items))
        .route("/sync/items/cancel", post(cancel_items))
        .route("/Sync/{item_id}/Status", post(report_status))
        .route("/sync/{item_id}/status", post(report_status))
        .route("/Sync/{target_id}/Items", delete(cancel_target_items))
        .route("/sync/{target_id}/items", delete(cancel_target_items))
        .route("/Sync/{target_id}/Items/Delete", post(cancel_target_items))
        .route("/sync/{target_id}/items/delete", post(cancel_target_items))
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
    target_id: String,
}

#[derive(Debug)]
struct AdditionalFileQuery {
    _name: String,
}

#[derive(Debug)]
struct ItemIdsQuery {
    item_ids: Option<String>,
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
            target_id: required_string(deserializer, "TargetId")?,
        })
    }
}

impl<'de> Deserialize<'de> for AdditionalFileQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            _name: required_string(deserializer, "Name")?,
        })
    }
}

impl<'de> Deserialize<'de> for ItemIdsQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = ItemIdsQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an optional Emby ItemIds query")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut item_ids = None;
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("ItemIds") {
                        item_ids = Some(map.next_value::<String>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(ItemIdsQuery { item_ids })
            }
        }

        deserializer.deserialize_map(Visitor)
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SyncDataResponse {
    item_ids_to_remove: Vec<String>,
}

async fn targets(Query(_query): Query<UserIdQuery>) -> Json<Vec<Value>> {
    Json(Vec::new())
}

async fn jobs() -> Json<QueryResult> {
    empty_query_result()
}

async fn create_job(
    request: Result<Json<Map<String, Value>>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    require_json(request)?;
    Ok(StatusCode::NOT_FOUND)
}

// The historical controller delegated these lookups to ISyncManager. With no
// registered legacy sync provider there cannot be a matching job, job item,
// or provider-owned output path, so a 404 is the only honest result.
async fn sync_job(Path(_id): Path<String>) -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn update_job(
    Path(_id): Path<i64>,
    request: Result<Json<Map<String, Value>>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    require_json(request)?;
    Ok(StatusCode::NOT_FOUND)
}

async fn unavailable_id(Path(_id): Path<String>) -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn report_status(
    Path(_item_id): Path<String>,
    request: Result<Json<Map<String, Value>>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    require_json(request)?;
    Ok(StatusCode::NOT_FOUND)
}

async fn offline_actions(
    request: Result<Json<Vec<Map<String, Value>>>, JsonRejection>,
) -> Result<StatusCode, StatusCode> {
    require_json(request)?;
    Ok(StatusCode::NOT_FOUND)
}

async fn sync_data(
    query: Result<Query<TargetIdQuery>, QueryRejection>,
    request: Result<Json<Map<String, Value>>, JsonRejection>,
) -> Result<Json<SyncDataResponse>, StatusCode> {
    let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
    let _ = query.target_id;
    require_json(request)?;
    Ok(Json(SyncDataResponse {
        item_ids_to_remove: Vec::new(),
    }))
}

async fn cancel_items(
    query: Result<Query<ItemIdsQuery>, QueryRejection>,
) -> Result<StatusCode, StatusCode> {
    let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
    let _ = query.item_ids;
    Ok(StatusCode::NOT_FOUND)
}

async fn cancel_target_items(
    Path(_target_id): Path<String>,
    query: Result<Query<ItemIdsQuery>, QueryRejection>,
) -> Result<StatusCode, StatusCode> {
    let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
    let _ = query.item_ids;
    Ok(StatusCode::NOT_FOUND)
}

fn require_json<T>(request: Result<Json<T>, JsonRejection>) -> Result<T, StatusCode> {
    request
        .map(|Json(value)| value)
        .map_err(|_| StatusCode::BAD_REQUEST)
}

async fn job_item_file(Path(_id): Path<String>) -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn job_item_additional_file(
    Path(_id): Path<String>,
    Query(_query): Query<AdditionalFileQuery>,
) -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn job_items(Query(query): Query<TargetIdQuery>) -> Json<QueryResult> {
    let _ = query.target_id;
    empty_query_result()
}

async fn ready_items(Query(query): Query<TargetIdQuery>) -> Json<Vec<Value>> {
    let _ = query.target_id;
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
        http::{Method, Request, StatusCode, header},
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
    async fn unavailable_object_and_file_lookups_are_not_found() {
        for path in [
            "/Sync/Jobs/job-id",
            "/Sync/JobItems/item-id/File",
            "/Sync/JobItems/item-id/AdditionalFiles?nAmE=first&NAME=second",
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let response = app()
            .oneshot(
                Request::head("/Sync/JobItems/item-id/File")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app()
            .oneshot(
                Request::get("/Sync/JobItems/item-id/AdditionalFiles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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

    #[tokio::test]
    async fn sync_mutations_bind_requests_before_reporting_provider_unavailable() {
        for path in [
            "/Sync/Jobs",
            "/Sync/OfflineActions",
            "/Sync/Data?TargetId=target",
            "/Sync/item/Status",
            "/Sync/Jobs/1",
        ] {
            let response = mutation(Method::POST, path, None).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        }

        assert_eq!(
            mutation(Method::POST, "/Sync/Data", Some("{}"))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "TargetId is required before Sync/Data can answer",
        );
        assert_eq!(
            mutation(Method::POST, "/Sync/Jobs/not-an-int", Some("{}"))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "the generated update path binds Id as Int64",
        );
        assert_eq!(
            mutation(Method::POST, "/Sync/OfflineActions", Some("{}"))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "offline actions require a JSON array body",
        );
    }

    #[tokio::test]
    async fn all_generated_sync_mutations_have_lowercase_routes_and_honest_results() {
        let unavailable = [
            (Method::POST, "/sync/jobs", Some("{}")),
            (Method::POST, "/sync/offlineactions", Some("[]")),
            (Method::POST, "/sync/item/status", Some("{}")),
            (Method::POST, "/sync/jobs/1", Some("{}")),
            (Method::DELETE, "/sync/jobs/job", None),
            (Method::POST, "/sync/items/cancel", None),
            (Method::DELETE, "/sync/target/items", None),
            (Method::DELETE, "/sync/jobitems/item", None),
            (Method::POST, "/sync/jobs/job/delete", None),
            (Method::POST, "/sync/target/items/delete", None),
            (Method::POST, "/sync/jobitems/item/transferred", None),
            (Method::POST, "/sync/jobitems/item/enable", None),
            (Method::POST, "/sync/jobitems/item/delete", None),
            (Method::POST, "/sync/jobitems/item/markforremoval", None),
            (Method::POST, "/sync/jobitems/item/unmarkforremoval", None),
        ];
        for (method, path, body) in unavailable {
            let response = mutation(method.clone(), path, body).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        }

        let response = mutation(
            Method::POST,
            "/sync/data?tArGeTiD=first&TARGETID=second",
            Some(r#"{"LocalItemIds":[],"InternalTargetIds":[]}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(
                &to_bytes(response.into_body(), 1024)
                    .await
                    .expect("Sync/Data body")
            )
            .expect("Sync/Data JSON"),
            serde_json::json!({"ItemIdsToRemove": []})
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

    #[test]
    fn mutation_queries_bind_case_insensitively_and_keep_the_last_duplicate() {
        let target: TargetIdQuery =
            serde_json::from_str(r#"{"TargetId":"first","targetid":"second"}"#)
                .expect("TargetId query");
        assert_eq!(target.target_id, "second");

        let items: ItemIdsQuery =
            serde_json::from_str(r#"{"ItemIds":"first","ignored":true,"itemids":"second"}"#)
                .expect("ItemIds query");
        assert_eq!(items.item_ids.as_deref(), Some("second"));
    }

    async fn mutation(method: Method, path: &str, body: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(path);
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        app()
            .oneshot(
                request
                    .body(Body::from(body.unwrap_or_default().to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}
