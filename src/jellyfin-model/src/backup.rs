use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BackupOptionsDto {
    pub metadata: bool,
    pub trickplay: bool,
    pub subtitles: bool,
    pub database: bool,
}

impl Default for BackupOptionsDto {
    fn default() -> Self {
        Self {
            metadata: false,
            trickplay: false,
            subtitles: false,
            database: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BackupManifestDto {
    pub server_version: String,
    pub backup_engine_version: String,
    #[serde(with = "crate::serde_datetime::required")]
    pub date_created: DateTime<Utc>,
    pub path: String,
    pub options: BackupOptionsDto,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BackupRestoreRequestDto {
    pub archive_file_name: String,
}

impl Default for BackupManifestDto {
    fn default() -> Self {
        Self {
            server_version: String::new(),
            backup_engine_version: String::new(),
            date_created: DateTime::<Utc>::UNIX_EPOCH,
            path: String::new(),
            options: BackupOptionsDto::default(),
        }
    }
}
