use serde::{Deserialize, Serialize};

/// The kind of an entry exposed by the server file-system browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum FileSystemEntryType {
    File,
    Directory,
    NetworkComputer,
    NetworkShare,
}

/// Public file-system entry returned by the environment API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct FileSystemEntryInfo {
    pub name: String,
    pub path: String,
    #[serde(rename = "Type")]
    pub entry_type: FileSystemEntryType,
}

impl FileSystemEntryInfo {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        path: impl Into<String>,
        entry_type: FileSystemEntryType,
    ) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            entry_type,
        }
    }

    #[must_use]
    pub fn directory(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self::new(name, path, FileSystemEntryType::Directory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_system_entry_uses_the_official_json_shape() {
        let entry = FileSystemEntryInfo::directory("Media", "/srv/media");

        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            serde_json::json!({
                "Name": "Media",
                "Path": "/srv/media",
                "Type": "Directory"
            })
        );
    }
}
