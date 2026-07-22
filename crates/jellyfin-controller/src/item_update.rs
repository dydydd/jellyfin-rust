use std::collections::{BTreeMap, HashSet};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ServerConfigurationRepository, ServerConfigurationStoreError,
    entities::base_item,
};
use sea_orm::DatabaseConnection;
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
}

/// Applies Jellyfin item-editor normalization before `PostgreSQL` persistence.
#[derive(Clone)]
pub struct ItemUpdateService {
    repository: ItemUpdateRepository,
    items: BaseItemRepository,
    server_configuration: ServerConfigurationRepository,
}

impl ItemUpdateService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            repository: ItemUpdateRepository::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            server_configuration: ServerConfigurationRepository::new(database),
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
        Ok(self
            .repository
            .update(item_id, normalize_input(input))
            .await?)
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
}

fn containing_folder_path(item: &base_item::Model) -> String {
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
            index_number: None,
            parent_index_number: None,
            production_year: None,
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
}
