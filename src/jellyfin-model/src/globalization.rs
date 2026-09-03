use serde::{Deserialize, Serialize};

/// A selectable server UI culture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct LocalizationOption {
    pub name: String,
    pub value: String,
}
