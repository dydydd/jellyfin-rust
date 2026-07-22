use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{UserConfiguration, UserPolicy};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UserDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(with = "crate::serde_guid::single")]
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_configured_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_configured_easy_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_auto_login: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub last_login_date: Option<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub last_activity_date: Option<DateTime<Utc>>,
    pub configuration: UserConfiguration,
    pub policy: UserPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,
}

impl Default for UserDto {
    fn default() -> Self {
        Self {
            name: None,
            server_id: None,
            server_name: None,
            id: Uuid::nil(),
            primary_image_tag: None,
            has_password: Some(true),
            has_configured_password: Some(true),
            has_configured_easy_password: Some(false),
            enable_auto_login: None,
            last_login_date: None,
            last_activity_date: None,
            configuration: UserConfiguration::default(),
            policy: UserPolicy::default(),
            primary_image_aspect_ratio: None,
        }
    }
}

impl std::fmt::Display for UserDto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => formatter.write_str(name),
            None => formatter.write_str(&self.id.simple().to_string()),
        }
    }
}
