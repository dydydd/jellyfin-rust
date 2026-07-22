use std::str::FromStr;

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ServerConfigurationRepository,
    ServerConfigurationStoreError, VirtualFolderError, VirtualFolderRepository,
    entities::base_item,
};
use jellyfin_model::{CollectionType, MetadataEditorInfo, NameValuePair};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{LocalizationService, item_update::containing_folder_path};

const CONTENT_TYPE_OPTIONS: &[(&str, &str)] = &[
    ("Inherit", ""),
    ("Movies", "movies"),
    ("Music", "music"),
    ("Shows", "tvshows"),
    ("HomeVideos", "homevideos"),
    ("MusicVideos", "musicvideos"),
    ("Photos", "photos"),
];

#[derive(Debug, Error)]
pub enum MetadataEditorError {
    #[error("item was not found")]
    NotFound,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ServerConfiguration(#[from] ServerConfigurationStoreError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
}

/// Builds metadata-editor contracts from persisted library and configuration
/// state plus Jellyfin's embedded localization and provider registries.
#[derive(Clone)]
pub struct MetadataEditorService {
    items: BaseItemRepository,
    server_configuration: ServerConfigurationRepository,
    virtual_folders: VirtualFolderRepository,
    localization: LocalizationService,
}

impl MetadataEditorService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            items: BaseItemRepository::new(database.clone()),
            server_configuration: ServerConfigurationRepository::new(database.clone()),
            virtual_folders: VirtualFolderRepository::new(database),
            localization: LocalizationService,
        }
    }

    /// Loads the item and every persisted input used by the official metadata
    /// editor response.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataEditorError::NotFound`] for an unknown item or the
    /// corresponding `PostgreSQL` persistence error.
    pub async fn get(&self, item_id: Uuid) -> Result<MetadataEditorInfo, MetadataEditorError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(MetadataEditorError::NotFound)?;
        let configuration = self.server_configuration.load().await?;
        let overrides = content_type_overrides(&configuration.content_types);
        let mut info = MetadataEditorInfo {
            parental_rating_options: self
                .localization
                .parental_ratings(&configuration.metadata_country_code),
            countries: self.localization.countries(),
            cultures: self.localization.distinct_sorted_cultures(),
            external_id_infos: jellyfin_providers::external_id::external_id_infos(&item.item_type),
            ..MetadataEditorInfo::default()
        };

        if content_type_editable(&item) {
            let path = containing_folder_path(&item);
            let configured = configured_content_type(&overrides, &path);
            let inherited = self.inherited_content_type(&item, &overrides).await?;
            if inherited.is_none() || configured.is_some() {
                info.content_type = configured;
                info.content_type_options = content_type_options();
                if inherited.is_none()
                    || matches!(
                        inherited,
                        Some(CollectionType::Movies | CollectionType::TvShows)
                    )
                {
                    info.content_type_options.retain(|option| {
                        option.value.is_empty()
                            || option.value.eq_ignore_ascii_case("movies")
                            || option.value.eq_ignore_ascii_case("tvshows")
                    });
                }
            }
        }
        Ok(info)
    }

    async fn inherited_content_type(
        &self,
        item: &base_item::Model,
        overrides: &[(String, CollectionType)],
    ) -> Result<Option<CollectionType>, MetadataEditorError> {
        let item_path = item.path.as_deref().unwrap_or_default();
        for folder in self.virtual_folders.list().await? {
            if folder
                .paths
                .iter()
                .any(|media_path| path_contains(&media_path.path, item_path))
                && let Some(collection_type) = folder
                    .folder
                    .collection_type
                    .as_deref()
                    .and_then(|value| CollectionType::from_str(value).ok())
            {
                return Ok(Some(collection_type));
            }
        }

        let mut inherited = None;
        for ancestor in self.items.ancestors(item.id).await? {
            if let Some(configured) =
                configured_content_type(overrides, &containing_folder_path(&ancestor.item))
            {
                inherited = Some(configured);
            }
        }
        Ok(inherited)
    }
}

fn content_type_overrides(value: &Value) -> Vec<(String, CollectionType)> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get("Name")?.as_str()?.trim();
            let collection_type = CollectionType::from_str(entry.get("Value")?.as_str()?).ok()?;
            (!name.is_empty()).then(|| (name.to_owned(), collection_type))
        })
        .collect()
}

fn configured_content_type(
    overrides: &[(String, CollectionType)],
    path: &str,
) -> Option<CollectionType> {
    overrides
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(path))
        .map(|(_, collection_type)| *collection_type)
}

fn content_type_options() -> Vec<NameValuePair> {
    CONTENT_TYPE_OPTIONS
        .iter()
        .map(|(name, value)| NameValuePair {
            name: (*name).to_owned(),
            value: (*value).to_owned(),
        })
        .collect()
}

fn content_type_editable(item: &base_item::Model) -> bool {
    !item.is_virtual_item
        && !matches!(
            item.item_type.as_str(),
            "CollectionFolder"
                | "UserView"
                | "AggregateFolder"
                | "LiveTvChannel"
                | "Genre"
                | "MusicArtist"
                | "MusicGenre"
                | "Person"
                | "Studio"
                | "Year"
        )
        && item.data.as_ref().is_none_or(source_is_library)
}

fn source_is_library(data: &Value) -> bool {
    if data
        .get("ChannelId")
        .and_then(Value::as_str)
        .is_some_and(|channel_id| !channel_id.is_empty())
    {
        return false;
    }
    match data.get("SourceType") {
        None => true,
        Some(Value::String(source_type)) => source_type.eq_ignore_ascii_case("Library"),
        Some(Value::Number(source_type)) => source_type.as_i64() == Some(0),
        Some(_) => false,
    }
}

fn path_contains(root: &str, candidate: &str) -> bool {
    let root = normalized_path(root);
    let candidate = normalized_path(candidate);
    candidate == root
        || candidate
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_match_case_insensitively_only_at_directory_boundaries() {
        assert!(path_contains("/Media/Movies", "/media/movies/Title.mkv"));
        assert!(path_contains(
            r"C:\Media\Movies",
            r"c:\media\movies\Title.mkv"
        ));
        assert!(!path_contains("/media/movie", "/media/movies/title.mkv"));
    }

    #[test]
    fn only_library_source_metadata_is_content_type_editable() {
        assert!(source_is_library(&json!({})));
        assert!(source_is_library(&json!({ "SourceType": "library" })));
        assert!(source_is_library(&json!({ "SourceType": 0 })));
        assert!(!source_is_library(&json!({ "SourceType": "Channel" })));
        assert!(!source_is_library(&json!({ "SourceType": "LiveTV" })));
        assert!(!source_is_library(&json!({ "SourceType": 1 })));
        assert!(!source_is_library(&json!({
            "SourceType": "Library",
            "ChannelId": "a-channel"
        })));
    }
}
