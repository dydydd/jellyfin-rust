use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct FolderStorageDto {
    pub path: String,
    pub free_space: i64,
    pub used_space: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct LibraryStorageDto {
    #[cfg_attr(feature = "openapi", schemars(with = "String"))]
    #[serde(with = "crate::serde_guid::single")]
    pub id: Uuid,
    pub name: String,
    pub folders: Vec<FolderStorageDto>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct SystemStorageDto {
    pub program_data_folder: FolderStorageDto,
    pub web_folder: FolderStorageDto,
    pub image_cache_folder: FolderStorageDto,
    pub cache_folder: FolderStorageDto,
    pub log_folder: FolderStorageDto,
    pub internal_metadata_folder: FolderStorageDto,
    pub transcoding_temp_folder: FolderStorageDto,
    pub libraries: Vec<LibraryStorageDto>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "camelCase")]
pub struct PackageInfo {
    pub name: String,
    pub description: String,
    pub overview: String,
    pub owner: String,
    pub category: String,
    #[cfg_attr(feature = "openapi", schemars(with = "String"))]
    #[serde(rename = "guid", with = "crate::serde_guid::single")]
    pub id: Uuid,
    pub versions: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct InstallationInfo {
    #[cfg_attr(feature = "openapi", schemars(with = "String"))]
    #[serde(rename = "Guid", with = "crate::serde_guid::single")]
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub changelog: String,
    pub source_url: String,
    pub checksum: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_info: Option<PackageInfo>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct CastReceiverApplication {
    pub id: String,
    pub name: String,
}

/// Public, unauthenticated server information returned by `/System/Info/Public`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PublicSystemInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_name: Option<String>,
    /// Retained for wire compatibility; current Jellyfin servers return an empty string.
    pub operating_system: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_wizard_completed: Option<bool>,
}

/// Authenticated server information returned by `/System/Info`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct SystemInfo {
    #[serde(flatten)]
    pub public_info: PublicSystemInfo,
    /// Retained for wire compatibility; current Jellyfin servers return an empty string.
    pub operating_system_display_name: String,
    pub package_name: Option<String>,
    pub has_pending_restart: bool,
    pub is_shutting_down: bool,
    pub supports_library_monitor: bool,
    pub web_socket_port_number: i32,
    pub completed_installations: Vec<InstallationInfo>,
    pub can_self_restart: bool,
    pub can_launch_web_browser: bool,
    pub program_data_path: String,
    pub web_path: String,
    pub items_by_name_path: String,
    pub cache_path: String,
    pub log_path: String,
    pub internal_metadata_path: String,
    pub transcoding_temp_path: String,
    pub cast_receiver_applications: Vec<CastReceiverApplication>,
    pub has_update_available: bool,
    pub encoder_location: String,
    pub system_architecture: String,
}
