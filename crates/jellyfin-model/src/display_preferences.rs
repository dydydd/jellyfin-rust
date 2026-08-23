use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum ScrollDirection {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

/// Display preferences for a client/user/item tuple.
///
/// Jellyfin's official DTO is intentionally broad: clients store both
/// first-class display fields and arbitrary string custom preferences here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct DisplayPreferencesDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_by: Option<String>,
    pub remember_indexing: bool,
    pub primary_image_height: i32,
    pub primary_image_width: i32,
    pub custom_prefs: HashMap<String, Option<String>>,
    pub scroll_direction: ScrollDirection,
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    pub sort_order: SortOrder,
    pub show_sidebar: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
}

impl Default for DisplayPreferencesDto {
    fn default() -> Self {
        Self {
            id: None,
            view_type: None,
            sort_by: None,
            index_by: None,
            remember_indexing: false,
            primary_image_height: 250,
            primary_image_width: 250,
            custom_prefs: HashMap::new(),
            scroll_direction: ScrollDirection::Horizontal,
            show_backdrop: true,
            remember_sorting: false,
            sort_order: SortOrder::Ascending,
            show_sidebar: false,
            client: None,
        }
    }
}
