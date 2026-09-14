use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ClientCapabilitiesDto;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceInfoDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option"
    )]
    pub last_user_id: Option<Uuid>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub date_last_activity: Option<DateTime<Utc>>,
    pub capabilities: ClientCapabilitiesDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceOptionsDto {
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
}

impl<'de> Deserialize<'de> for DeviceOptionsDto {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = DeviceOptionsDto;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a device options object")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut normalized = serde_json::Map::new();
                while let Some(name) = map.next_key::<String>()? {
                    let value = map.next_value::<serde_json::Value>()?;
                    let canonical = if name.eq_ignore_ascii_case("Id") {
                        Some("Id")
                    } else if name.eq_ignore_ascii_case("DeviceId") {
                        Some("DeviceId")
                    } else if name.eq_ignore_ascii_case("CustomName") {
                        Some("CustomName")
                    } else {
                        None
                    };
                    if let Some(canonical) = canonical {
                        normalized.insert(canonical.to_owned(), value);
                    }
                }

                #[derive(Default, Deserialize)]
                #[serde(default, rename_all = "PascalCase")]
                struct Wire {
                    id: i32,
                    device_id: Option<String>,
                    custom_name: Option<String>,
                }

                let wire: Wire = serde_json::from_value(serde_json::Value::Object(normalized))
                    .map_err(serde::de::Error::custom)?;
                Ok(DeviceOptionsDto {
                    id: wire.id,
                    device_id: wire.device_id,
                    custom_name: wire.custom_name,
                })
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceOptionsDto;

    #[test]
    fn device_options_use_case_insensitive_last_values() {
        let options: DeviceOptionsDto = serde_json::from_str(
            r#"{
                "ID": "not-an-integer",
                "id": 42,
                "DEVICEID": false,
                "deviceid": "expected-device",
                "CUSTOMNAME": ["not-a-string"],
                "Unknown": {"Nested": true},
                "customname": "Expected Room"
            }"#,
        )
        .expect("device options");

        assert_eq!(options.id, 42);
        assert_eq!(options.device_id.as_deref(), Some("expected-device"));
        assert_eq!(options.custom_name.as_deref(), Some("Expected Room"));
    }
}
