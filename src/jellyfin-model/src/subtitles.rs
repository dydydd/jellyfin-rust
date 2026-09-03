use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FontFile {
    pub name: Option<String>,
    pub size: i64,
    #[serde(with = "crate::serde_datetime::required")]
    pub date_created: DateTime<Utc>,
    #[serde(with = "crate::serde_datetime::required")]
    pub date_modified: DateTime<Utc>,
}
