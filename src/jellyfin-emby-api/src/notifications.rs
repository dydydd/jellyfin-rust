//! Emby notification capability discovery and provider-owned operations.
//!
//! The Rust server does not currently expose an Emby notification provider.
//! Capability discovery therefore stays empty, provider defaults fail with
//! the same legacy service error as an empty official provider registry, and
//! the two fire-and-forget operations retain Emby's empty-response behavior
//! without claiming that a provider delivered anything.

use std::{collections::HashMap, fmt, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use serde::{Deserialize, Deserializer, Serialize, de};
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Notifications/Types", get(notification_types))
        .route("/notifications/types", get(notification_types))
        .route("/Notifications/Admin", post(add_admin_notification))
        .route("/Notifications/Services/Test", post(send_test_notification))
        .route(
            "/Notifications/Services/Defaults",
            get(default_notification_info),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NotificationCategoryInfo {
    name: Option<String>,
    id: Option<String>,
    events: Option<Vec<NotificationTypeInfo>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct NotificationTypeInfo {
    name: Option<String>,
    id: Option<String>,
    category_id: Option<String>,
    category_name: Option<String>,
}

async fn notification_types() -> Json<Vec<NotificationCategoryInfo>> {
    Json(Vec::new())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AdminNotificationBody {
    display_date_time: Option<bool>,
}

impl<'de> Deserialize<'de> for AdminNotificationBody {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = AdminNotificationBody;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby AddAdminNotification object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut body = AdminNotificationBody::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("DisplayDateTime") {
                        body.display_date_time = map.next_value::<Option<bool>>()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(body)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct UserNotificationInfo {
    notifier_key: Option<String>,
    setup_module_url: Option<String>,
    service_name: Option<String>,
    plugin_id: Option<String>,
    friendly_name: Option<String>,
    id: Option<String>,
    enabled: Option<bool>,
    user_ids: Option<Vec<String>>,
    device_ids: Option<Vec<String>>,
    library_ids: Option<Vec<String>>,
    event_ids: Option<Vec<String>>,
    user_id: Option<String>,
    is_self_notification: Option<bool>,
    group_items: Option<bool>,
    options: Option<HashMap<String, String>>,
}

impl<'de> Deserialize<'de> for UserNotificationInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UserNotificationInfo;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby UserNotificationInfo object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut info = UserNotificationInfo::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("NotifierKey") {
                        info.notifier_key = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("SetupModuleUrl") {
                        info.setup_module_url = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("ServiceName") {
                        info.service_name = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("PluginId") {
                        info.plugin_id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("FriendlyName") {
                        info.friendly_name = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("Id") {
                        info.id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("Enabled") {
                        info.enabled = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("UserIds") {
                        info.user_ids = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("DeviceIds") {
                        info.device_ids = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("LibraryIds") {
                        info.library_ids = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("EventIds") {
                        info.event_ids = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("UserId") {
                        info.user_id = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("IsSelfNotification") {
                        info.is_self_notification = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("GroupItems") {
                        info.group_items = map.next_value()?;
                    } else if name.eq_ignore_ascii_case("Options") {
                        info.options = map.next_value()?;
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(info)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AdminNotificationQuery {
    name: Option<String>,
    description: Option<String>,
    image_url: Option<String>,
    url: Option<String>,
    level: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DefaultNotificationQuery {
    user_id: Option<String>,
    notifier_key: Option<String>,
}

fn parse_admin_query(uri: &axum::http::Uri) -> AdminNotificationQuery {
    let mut query = AdminNotificationQuery::default();
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("Name") {
            query.name = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("Description") {
            query.description = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("ImageUrl") {
            query.image_url = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("Url") {
            query.url = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("Level") {
            query.level = Some(value.into_owned());
        }
    }
    query
}

fn parse_default_query(uri: &axum::http::Uri) -> DefaultNotificationQuery {
    let mut query = DefaultNotificationQuery::default();
    for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        if name.eq_ignore_ascii_case("UserId") {
            query.user_id = Some(value.into_owned());
        } else if name.eq_ignore_ascii_case("NotifierKey") {
            query.notifier_key = Some(value.into_owned());
        }
    }
    query
}

fn parse_optional_body<T>(body: &[u8]) -> T
where
    T: for<'de> Deserialize<'de> + Default,
{
    if body.is_empty() {
        return T::default();
    }
    // ServiceStack's legacy request DTO binder treats a missing or malformed
    // JSON document as the DTO's default value. This is observable on both
    // official void operations, which still return 204 for a truncated body.
    serde_json::from_slice(body).unwrap_or_default()
}

async fn add_admin_notification(OriginalUri(uri): OriginalUri, body: Bytes) -> StatusCode {
    let _query = parse_admin_query(&uri);
    let _body = parse_optional_body::<AdminNotificationBody>(&body);
    // The official operation is fire-and-forget and is a no-op when no
    // configured notifier accepts the event. `204` is its actual wire status.
    StatusCode::NO_CONTENT
}

async fn send_test_notification(body: Bytes) -> StatusCode {
    let _body = parse_optional_body::<UserNotificationInfo>(&body);
    // Official Emby also completes successfully when NotifierKey is omitted
    // or does not identify a provider. Do not synthesize a delivery result.
    StatusCode::NO_CONTENT
}

async fn default_notification_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let query = parse_default_query(&uri);
    let Some(user_id) = query.user_id else {
        return legacy_notification_error("Object reference not set to an instance of an object.");
    };
    let Ok(user_id) = Uuid::parse_str(&user_id) else {
        return legacy_notification_error(
            "Guid should contain 32 digits with 4 dashes (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx).",
        );
    };
    let target = match state
        .emby_notification_target_is_administrator(user_id)
        .await
    {
        Ok(target) => target,
        Err(response) => return response,
    };
    let Some(_is_administrator) = target else {
        return legacy_notification_error("Object reference not set to an instance of an object.");
    };

    // In official 4.10 this is `NotificationServices.First(...)`. With the
    // Rust server's empty provider registry, every key (including an omitted
    // key) therefore has the same observable failure.
    let _notifier_key = query.notifier_key;
    legacy_notification_error("Sequence contains no matching element")
}

fn legacy_notification_error(message: &'static str) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn notification_types_are_a_generated_client_decodable_empty_array() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));

        for path in ["/Notifications/Types", "/notifications/types"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK, "{path}");
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                serde_json::json!([])
            );
        }
    }

    #[test]
    fn notification_queries_are_case_insensitive_and_last_duplicate_wins() {
        let uri = "/Notifications/Admin?Name=first&nAmE=last&DESCRIPTION=body&ImageUrl=one&imageurl=two&URL=target&level=Warning"
            .parse()
            .unwrap();
        assert_eq!(
            parse_admin_query(&uri),
            AdminNotificationQuery {
                name: Some("last".to_owned()),
                description: Some("body".to_owned()),
                image_url: Some("two".to_owned()),
                url: Some("target".to_owned()),
                level: Some("Warning".to_owned()),
            }
        );

        let uri = "/Notifications/Services/Defaults?UserId=first&uSeRiD=last&NotifierKey=one&NOTIFIERKEY=two"
            .parse()
            .unwrap();
        assert_eq!(
            parse_default_query(&uri),
            DefaultNotificationQuery {
                user_id: Some("last".to_owned()),
                notifier_key: Some("two".to_owned()),
            }
        );
    }

    #[test]
    fn notification_bodies_are_case_insensitive_last_wins_and_typed() {
        let admin: AdminNotificationBody =
            serde_json::from_str(r#"{"DisplayDateTime":true,"unknown":1,"displaydatetime":false}"#)
                .unwrap();
        assert_eq!(admin.display_date_time, Some(false));

        let info: UserNotificationInfo = serde_json::from_str(
            r#"{"NotifierKey":"first","NOTIFIERKEY":"last","Enabled":true,"UserIds":["one"],"Options":{"Url":"https://example.invalid"},"Unknown":{}}"#,
        )
        .unwrap();
        assert_eq!(info.notifier_key.as_deref(), Some("last"));
        assert_eq!(info.enabled, Some(true));
        assert_eq!(info.user_ids, Some(vec!["one".to_owned()]));
        assert_eq!(
            info.options.unwrap().get("Url").map(String::as_str),
            Some("https://example.invalid")
        );

        assert!(
            serde_json::from_str::<AdminNotificationBody>(r#"{"DisplayDateTime":"true"}"#).is_err()
        );
        for invalid in [
            r#"{"NotifierKey":1}"#,
            r#"{"Enabled":"true"}"#,
            r#"{"UserIds":[1]}"#,
            r#"{"Options":{"Url":1}}"#,
        ] {
            assert!(serde_json::from_str::<UserNotificationInfo>(invalid).is_err());
        }

        assert_eq!(
            parse_optional_body::<AdminNotificationBody>(b"{"),
            AdminNotificationBody::default()
        );
        assert_eq!(
            parse_optional_body::<UserNotificationInfo>(br#"{"Enabled":"true"}"#),
            UserNotificationInfo::default()
        );
    }
}
