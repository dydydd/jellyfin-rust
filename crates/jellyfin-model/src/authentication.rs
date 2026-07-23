use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationInfo {
    pub id: i64,
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub app_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(with = "crate::serde_guid::single")]
    pub user_id: Uuid,
    pub is_active: bool,
    #[serde(with = "crate::serde_datetime::required")]
    pub date_created: DateTime<Utc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub date_revoked: Option<DateTime<Utc>>,
    #[serde(with = "crate::serde_datetime::required")]
    pub date_last_activity: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult<T> {
    pub items: Vec<T>,
    pub total_record_count: usize,
    pub start_index: usize,
}

impl<T> QueryResult<T> {
    #[must_use]
    pub fn from_items(items: Vec<T>) -> Self {
        Self {
            total_record_count: items.len(),
            items,
            start_index: 0,
        }
    }
}
