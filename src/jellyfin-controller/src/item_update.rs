use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ServerConfigurationRepository, ServerConfigurationStoreError,
    VirtualFolderError, VirtualFolderRepository, entities::base_item,
};
use jellyfin_xbmc_metadata::{
    MovieNfo, MovieNfoLocation, MovieVideoType, movie_nfo_save_paths, parse_movie_nfo_file,
    save_movie_nfo,
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

/// Three-state collection input used by the item metadata editor.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemUpdateInput {
    pub tags: Option<Vec<String>>,
    pub genres: Option<Vec<String>>,
    pub provider_ids: Option<BTreeMap<String, Option<String>>>,
}

#[derive(Debug, Error)]
pub enum ItemUpdateError {
    #[error(transparent)]
    Store(#[from] ItemUpdateStoreError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ServerConfiguration(#[from] ServerConfigurationStoreError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderError),
}

/// Applies Jellyfin item-editor normalization before `PostgreSQL` persistence.
#[derive(Clone)]
pub struct ItemUpdateService {
    repository: ItemUpdateRepository,
    items: BaseItemRepository,
    server_configuration: ServerConfigurationRepository,
    virtual_folders: VirtualFolderRepository,
}

impl ItemUpdateService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            repository: ItemUpdateRepository::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            server_configuration: ServerConfigurationRepository::new(std::sync::Arc::clone(
                &database,
            )),
            virtual_folders: VirtualFolderRepository::new(database),
        }
    }

    /// Updates only metadata collections supplied by the request.
    ///
    /// # Errors
    ///
    /// Returns persistence and metadata validation errors.
    pub async fn update(
        &self,
        item_id: Uuid,
        input: ItemUpdateInput,
    ) -> Result<base_item::Model, ItemUpdateError> {
        let updated = self
            .repository
            .update(item_id, normalize_input(input))
            .await?;
        if self.save_local_metadata_enabled(&updated).await?
            && let Err(error) = Self::write_local_nfo(&updated)
        {
            tracing::warn!(%error, "local NFO writeback failed");
        }
        Ok(updated)
    }

    async fn save_local_metadata_enabled(
        &self,
        item: &base_item::Model,
    ) -> Result<bool, ItemUpdateError> {
        let Some(item_path) = item.path.as_deref() else {
            return Ok(false);
        };
        for folder in self.virtual_folders.list().await? {
            let contained = folder
                .paths
                .iter()
                .any(|path| Path::new(item_path).starts_with(Path::new(&path.path)));
            if contained {
                return Ok(folder
                    .folder
                    .library_options
                    .get("SaveLocalMetadata")
                    .and_then(Value::as_bool)
                    .unwrap_or(false));
            }
        }
        Ok(false)
    }

    /// Replaces or removes the content-type override for an item's containing
    /// folder path.
    ///
    /// # Errors
    ///
    /// Returns [`BaseItemError::NotFound`] when the requested item does not
    /// exist, or a persistence error when the configuration cannot be saved.
    pub async fn update_content_type(
        &self,
        item_id: Uuid,
        content_type: Option<&str>,
    ) -> Result<(), ItemUpdateError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        let path = containing_folder_path(&item);
        let content_type = content_type.filter(|value| !value.trim().is_empty());
        self.server_configuration
            .update_content_type_override(&path, content_type)
            .await?;
        Ok(())
    }

    pub(crate) fn write_local_nfo(item: &base_item::Model) -> std::io::Result<()> {
        if !matches!(
            item.item_type.as_str(),
            "Movie" | "Video" | "Trailer" | "MusicVideo"
        ) {
            return Ok(());
        }
        let Some(path) = item.path.as_deref() else {
            return Ok(());
        };
        let Some(nfo_path) = movie_nfo_save_paths(&MovieNfoLocation {
            path: PathBuf::from(path),
            is_in_mixed_folder: false,
            video_type: MovieVideoType::File,
        })
        .into_iter()
        .next() else {
            return Ok(());
        };
        if let Some(parent) = nfo_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let existing = parse_movie_nfo_file(&nfo_path).unwrap_or_default();
        save_movie_nfo(&nfo_path, &movie_nfo_from_item(item, existing))
    }
}

pub(crate) fn movie_nfo_from_item(item: &base_item::Model, mut existing: MovieNfo) -> MovieNfo {
    let data = item.data.as_ref().and_then(serde_json::Value::as_object);
    if let Some(name) = item.name.as_deref().filter(|name| !name.is_empty()) {
        existing.name = Some(name.to_owned());
    }
    if let Some(overview) = item.overview.as_deref().filter(|value| !value.is_empty()) {
        existing.overview = Some(overview.to_owned());
    }
    let original_title = data
        .and_then(|data| data.get("OriginalTitle"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| existing.original_title.take())
        // ALLOW: the NFO schema owns display and original-title fields independently.
        .or_else(|| item.name.clone());
    existing.original_title = original_title;
    if let Some(production_year) = item.production_year {
        existing.production_year = Some(production_year);
    }
    if let Some(premiere_date) = item.premiere_date {
        existing.premiere_date = Some(premiere_date.date_naive());
    }
    if let Some(runtime_ticks) = item.runtime_ticks {
        existing.runtime_ticks = Some(runtime_ticks);
    }
    if let Some(official_rating) = item
        .official_rating
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        existing.official_rating = Some(official_rating.to_owned());
    }
    if let Some(is_locked) = data
        .and_then(|data| data.get("IsLocked"))
        .and_then(Value::as_bool)
    {
        existing.is_locked = is_locked;
    }
    if let Some(locked_fields) = data
        .and_then(|data| data.get("LockedFields"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
    {
        existing.locked_fields = locked_fields;
    }
    if let Some(genres) = data
        .and_then(|data| data.get("Genres"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
    {
        existing.genres = genres;
    }
    if let Some(provider_ids) = data
        .and_then(|data| data.get("ProviderIds"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
    {
        existing.provider_ids = provider_ids;
    }
    existing
}

pub(crate) fn containing_folder_path(item: &base_item::Model) -> String {
    let path = item.path.as_deref().unwrap_or_default();
    if item.is_folder {
        return path.to_owned();
    }

    let Some(separator) = path.rfind(['/', '\\']) else {
        return String::new();
    };
    if separator == 0 {
        return path[..1].to_owned();
    }
    if separator == 2 && path.as_bytes().get(1) == Some(&b':') {
        return path[..=separator].to_owned();
    }
    path[..separator].to_owned()
}

fn normalize_input(input: ItemUpdateInput) -> ItemMetadataPatch {
    ItemMetadataPatch {
        tags: input.tags.map(|values| distinct_ignore_case(values, true)),
        genres: input
            .genres
            .map(|values| distinct_ignore_case(values, false)),
        provider_ids: input.provider_ids.map(|values| {
            values
                .into_iter()
                .filter_map(|(key, value)| {
                    value
                        .filter(|value| !value.is_empty())
                        .map(|value| (key, value))
                })
                .collect()
        }),
    }
}

fn distinct_ignore_case(values: Vec<String>, trim: bool) -> Vec<String> {
    let mut seen = HashSet::with_capacity(values.len());
    values
        .into_iter()
        .filter_map(|value| {
            let value = if trim { value.trim().to_owned() } else { value };
            let folded = value
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            seen.insert(folded).then_some(value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_states_and_official_normalization_are_preserved() {
        let omitted = normalize_input(ItemUpdateInput::default());
        assert_eq!(omitted, ItemMetadataPatch::default());

        let empty = normalize_input(ItemUpdateInput {
            tags: Some(Vec::new()),
            genres: Some(Vec::new()),
            provider_ids: Some(BTreeMap::new()),
        });
        assert_eq!(empty.tags, Some(Vec::new()));
        assert_eq!(empty.genres, Some(Vec::new()));
        assert_eq!(empty.provider_ids, Some(BTreeMap::new()));

        let normalized = normalize_input(ItemUpdateInput {
            tags: Some(vec![
                "  New-Tag  ".to_owned(),
                "new-tag".to_owned(),
                "Other".to_owned(),
            ]),
            genres: Some(vec![
                "Action".to_owned(),
                "ACTION".to_owned(),
                " Épopée ".to_owned(),
                " éPOPÉE ".to_owned(),
            ]),
            provider_ids: None,
        });
        assert_eq!(
            normalized.tags,
            Some(vec!["New-Tag".to_owned(), "Other".to_owned()])
        );
        assert_eq!(
            normalized.genres,
            Some(vec!["Action".to_owned(), " Épopée ".to_owned()])
        );
    }

    #[test]
    fn provider_ids_drop_only_null_and_empty_values() {
        let normalized = normalize_input(ItemUpdateInput {
            provider_ids: Some(BTreeMap::from([
                ("Imdb".to_owned(), Some("tt1234567".to_owned())),
                ("Null".to_owned(), None),
                ("Empty".to_owned(), Some(String::new())),
                ("Whitespace".to_owned(), Some("  ".to_owned())),
            ])),
            ..Default::default()
        });
        assert_eq!(
            normalized.provider_ids,
            Some(BTreeMap::from([
                ("Imdb".to_owned(), "tt1234567".to_owned()),
                ("Whitespace".to_owned(), "  ".to_owned()),
            ]))
        );
    }

    #[test]
    fn containing_folder_path_matches_folder_and_file_semantics() {
        let mut item = base_item::Model {
            id: Uuid::nil(),
            item_type: "Movie".to_owned(),
            data: None,
            path: Some("/media/movies/title.mkv".to_owned()),
            parent_id: None,
            top_parent_id: None,
            name: None,
            clean_name: None,
            sort_name: None,
            media_type: None,
            overview: None,
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: chrono::DateTime::UNIX_EPOCH,
            date_modified: chrono::DateTime::UNIX_EPOCH,
            row_version: 1,
        };

        assert_eq!(containing_folder_path(&item), "/media/movies");
        item.path = Some(r"C:\Media\Movies\title.mkv".to_owned());
        assert_eq!(containing_folder_path(&item), r"C:\Media\Movies");
        item.path = Some("/library/movies".to_owned());
        item.is_folder = true;
        assert_eq!(containing_folder_path(&item), "/library/movies");
    }

    #[test]
    fn nfo_writer_merges_item_edits_without_dropping_existing_fields() {
        let existing = MovieNfo {
            original_title: Some("Old Original".to_owned()),
            tagline: Some("Keep this tagline".to_owned()),
            custom_rating: Some("Custom".to_owned()),
            is_locked: true,
            locked_fields: vec!["Cast".to_owned()],
            ..MovieNfo::default()
        };
        let item = base_item::Model {
            id: Uuid::new_v4(),
            item_type: "Movie".to_owned(),
            data: Some(serde_json::json!({
                "OriginalTitle": "New Original",
                "Genres": ["Action"],
                "ProviderIds": { "Imdb": "tt1234567" },
                "IsLocked": false,
                "LockedFields": ["Name"]
            })),
            path: Some("/media/movie.mkv".to_owned()),
            parent_id: None,
            top_parent_id: None,
            name: Some("Movie".to_owned()),
            clean_name: None,
            sort_name: None,
            media_type: None,
            overview: Some("Overview".to_owned()),
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: chrono::DateTime::UNIX_EPOCH,
            date_modified: chrono::DateTime::UNIX_EPOCH,
            row_version: 1,
        };

        let merged = movie_nfo_from_item(&item, existing);

        assert_eq!(merged.name.as_deref(), Some("Movie"));
        assert_eq!(merged.original_title.as_deref(), Some("New Original"));
        assert_eq!(merged.tagline.as_deref(), Some("Keep this tagline"));
        assert_eq!(merged.custom_rating.as_deref(), Some("Custom"));
        assert_eq!(merged.genres, ["Action"]);
        assert_eq!(merged.provider_ids["Imdb"], "tt1234567");
        assert!(!merged.is_locked);
        assert_eq!(merged.locked_fields, ["Name"]);
    }
}
