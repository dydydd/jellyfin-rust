use serde::{Deserialize, Serialize};

use crate::{configuration::ImageOption, providers::ImageType};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LibraryOptionsResultDto {
    pub metadata_savers: Vec<LibraryOptionInfoDto>,
    pub metadata_readers: Vec<LibraryOptionInfoDto>,
    pub subtitle_fetchers: Vec<LibraryOptionInfoDto>,
    pub lyric_fetchers: Vec<LibraryOptionInfoDto>,
    pub media_segment_providers: Vec<LibraryOptionInfoDto>,
    pub type_options: Vec<LibraryTypeOptionsDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LibraryTypeOptionsDto {
    #[serde(rename = "Type")]
    pub item_type: Option<String>,
    pub metadata_fetchers: Vec<LibraryOptionInfoDto>,
    pub image_fetchers: Vec<LibraryOptionInfoDto>,
    pub similar_item_providers: Vec<LibraryOptionInfoDto>,
    pub supported_image_types: Vec<ImageType>,
    pub default_image_options: Vec<ImageOption>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LibraryOptionInfoDto {
    pub name: Option<String>,
    pub default_enabled: bool,
}
