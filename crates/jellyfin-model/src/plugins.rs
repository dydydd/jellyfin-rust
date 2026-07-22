use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The load state Jellyfin reports for an installed plugin.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum PluginStatus {
    Restart = 1,
    #[default]
    Active = 0,
    Disabled = -1,
    NotSupported = -2,
    Malfunctioned = -3,
    Superseded = -4,
    Deleted = -5,
}

/// Public metadata for an installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_file_name: Option<String>,
    pub description: String,
    #[serde(with = "crate::serde_guid::single")]
    pub id: Uuid,
    pub can_uninstall: bool,
    pub has_image: bool,
    pub status: PluginStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_info_uses_the_official_json_shape() {
        let plugin = PluginInfo {
            name: "Bookshelf".to_owned(),
            version: "1.2.3.4".to_owned(),
            configuration_file_name: None,
            description: "A plugin".to_owned(),
            id: Uuid::from_u128(0x2d35_0a13_0bf7_4b61_859c_d5e6_01b5_facf),
            can_uninstall: true,
            has_image: false,
            status: PluginStatus::Active,
        };

        assert_eq!(
            serde_json::to_value(plugin).unwrap(),
            serde_json::json!({
                "Name": "Bookshelf",
                "Version": "1.2.3.4",
                "Description": "A plugin",
                "Id": "2d350a130bf74b61859cd5e601b5facf",
                "CanUninstall": true,
                "HasImage": false,
                "Status": "Active"
            })
        );
    }
}
