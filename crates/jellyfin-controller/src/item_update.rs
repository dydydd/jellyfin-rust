use std::collections::{BTreeMap, HashSet};

use jellyfin_data::{
    ItemMetadataPatch, ItemUpdateRepository, ItemUpdateStoreError, entities::base_item,
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
}

/// Applies Jellyfin item-editor normalization before `PostgreSQL` persistence.
#[derive(Clone)]
pub struct ItemUpdateService {
    repository: ItemUpdateRepository,
}

impl ItemUpdateService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            repository: ItemUpdateRepository::new(database),
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
}
