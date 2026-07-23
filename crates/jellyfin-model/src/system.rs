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
