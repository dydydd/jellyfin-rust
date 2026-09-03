use std::collections::BTreeMap;

use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ActiveEnum, ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseTransaction,
    DbBackend, DbErr, EntityTrait, IntoActiveModel, QuerySelect, Statement, TransactionTrait,
    sea_query::OnConflict,
};
use serde_json::{Map, Value};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, item_value, item_value_map};

/// Optional metadata collections supplied by an item-update request.
///
/// `None` preserves the stored value while `Some(Vec::new())` or an empty map
/// clears it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemMetadataPatch {
    pub tags: Option<Vec<String>>,
    pub genres: Option<Vec<String>>,
    pub provider_ids: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Error)]
pub enum ItemUpdateStoreError {
    #[error("base item was not found")]
    NotFound,
    #[error("base item metadata must be a JSON object")]
    InvalidMetadata,
    #[error("item metadata value has no searchable characters")]
    InvalidValue,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// Atomically updates editable item metadata and its normalized query values.
#[derive(Clone)]
pub struct ItemUpdateRepository {
    database: crate::SharedDatabase,
}

impl ItemUpdateRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Applies one partial metadata update under an item row lock.
    ///
    /// JSON metadata and normalized genre/tag mappings commit together. The
    /// row update also advances the existing `row_version` trigger exactly
    /// once, including collection-only edits.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid metadata/value, or database errors.
    pub async fn update(
        &self,
        item_id: Uuid,
        patch: ItemMetadataPatch,
    ) -> Result<base_item::Model, ItemUpdateStoreError> {
        let transaction = self.database.begin().await?;
        let item = base_item::Entity::find_by_id(item_id)
            .lock_exclusive()
            .one(&transaction)
            .await?
            .ok_or(ItemUpdateStoreError::NotFound)?;
        let mut item = item;

        if let Some(tags) = patch.tags.as_deref() {
            replace_values(&transaction, item_id, item_value::ItemValueType::Tags, tags).await?;
        }
        if let Some(genres) = patch.genres.as_deref() {
            replace_values(
                &transaction,
                item_id,
                item_value::ItemValueType::Genre,
                genres,
            )
            .await?;
        }
        let data = patch_data(std::mem::take(&mut item.data), patch)?;

        let mut active = item.into_active_model();
        active.data = Set(data);
        let updated = active.update(&transaction).await?;
        transaction.commit().await?;
        Ok(updated)
    }
}

fn patch_data(
    data: Option<Value>,
    patch: ItemMetadataPatch,
) -> Result<Option<Value>, ItemUpdateStoreError> {
    if patch.tags.is_none() && patch.genres.is_none() && patch.provider_ids.is_none() {
        return Ok(data);
    }
    let mut object = match data {
        None => Map::new(),
        Some(Value::Object(object)) => object,
        Some(_) => return Err(ItemUpdateStoreError::InvalidMetadata),
    };
    if let Some(tags) = patch.tags {
        object.insert(
            "Tags".to_owned(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(genres) = patch.genres {
        object.insert(
            "Genres".to_owned(),
            Value::Array(genres.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(provider_ids) = patch.provider_ids {
        object.insert(
            "ProviderIds".to_owned(),
            Value::Object(
                provider_ids
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }
    Ok(Some(Value::Object(object)))
}

async fn replace_values(
    transaction: &DatabaseTransaction,
    item_id: Uuid,
    value_type: item_value::ItemValueType,
    values: &[String],
) -> Result<(), ItemUpdateStoreError> {
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM jellyfin.item_value_map AS mapping \
             USING jellyfin.item_values AS value \
             WHERE mapping.item_value_id = value.item_value_id \
               AND mapping.item_id = $1 \
               AND value.type = $2",
            [item_id.into(), value_type.to_value().into()],
        ))
        .await?;

    for value in values {
        let Some((value, clean_value)) = normalized_mapping_value(value)? else {
            continue;
        };
        let stored = item_value::Entity::insert(item_value::ActiveModel {
            item_value_id: Set(Uuid::new_v4()),
            value_type: Set(value_type),
            value: Set(value),
            clean_value: Set(clean_value),
        })
        .on_conflict(
            OnConflict::columns([
                item_value::Column::ValueType,
                item_value::Column::CleanValue,
            ])
            .update_column(item_value::Column::CleanValue)
            .to_owned(),
        )
        .exec_with_returning(transaction)
        .await?;
        item_value_map::Entity::insert(item_value_map::ActiveModel {
            item_value_id: Set(stored.item_value_id),
            item_id: Set(item_id),
        })
        .on_conflict(
            OnConflict::columns([
                item_value_map::Column::ItemValueId,
                item_value_map::Column::ItemId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(transaction)
        .await?;
    }
    Ok(())
}

fn normalized_mapping_value(value: &str) -> Result<Option<(String, String)>, ItemUpdateStoreError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let clean_value = value.clean_value();
    if clean_value.is_empty() {
        return Err(ItemUpdateStoreError::InvalidValue);
    }
    Ok(Some((value.to_owned(), clean_value)))
}
