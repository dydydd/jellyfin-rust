use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait, sea_query::OnConflict,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, item_value, item_value_map};

#[derive(Debug, Error)]
pub enum ItemValueError {
    #[error("item value cannot be empty")]
    InvalidValue,
    #[error("base item was not found")]
    ItemNotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed normalized item values and their many-to-many base-item
/// associations.
#[derive(Clone)]
pub struct ItemValueRepository {
    database: DatabaseConnection,
}

impl ItemValueRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Inserts a canonical item value, or returns the existing row whose
    /// normalized value is equivalent.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn upsert(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<item_value::Model, ItemValueError> {
        upsert_on(&self.database, value_type, value).await
    }

    /// Finds a value using exact, case-sensitive display text.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn get_exact(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<Option<item_value::Model>, ItemValueError> {
        let value = validate_value(value)?;
        Ok(item_value::Entity::find()
            .filter(item_value::Column::ValueType.eq(value_type))
            .filter(item_value::Column::Value.eq(value))
            .one(&self.database)
            .await?)
    }

    /// Finds a value using Jellyfin's Unicode-aware clean-value rules.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn get_normalized(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<Option<item_value::Model>, ItemValueError> {
        let value = validate_value(value)?;
        let clean_value = value.clean_value();
        Ok(item_value::Entity::find()
            .filter(item_value::Column::ValueType.eq(value_type))
            .filter(item_value::Column::CleanValue.eq(clean_value))
            .one(&self.database)
            .await?)
    }

    /// Atomically creates or reuses a normalized value and links it to a base
    /// item. Repeated and concurrent links are idempotent.
    ///
    /// # Errors
    ///
    /// Returns `ItemNotFound` when the base item is absent, or a validation or
    /// database error.
    pub async fn link(
        &self,
        item_id: Uuid,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<item_value::Model, ItemValueError> {
        let transaction = self.database.begin().await?;
        if base_item::Entity::find_by_id(item_id)
            .one(&transaction)
            .await?
            .is_none()
        {
            return Err(ItemValueError::ItemNotFound);
        }
        let item_value = upsert_on(&transaction, value_type, value).await?;
        item_value_map::Entity::insert(item_value_map::ActiveModel {
            item_value_id: Set(item_value.item_value_id),
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
        .exec_without_returning(&transaction)
        .await?;
        transaction.commit().await?;
        Ok(item_value)
    }

    /// Loads values of one type attached to an item in normalized name order.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn values_for_item(
        &self,
        item_id: Uuid,
        value_type: item_value::ItemValueType,
    ) -> Result<Vec<item_value::Model>, ItemValueError> {
        let ids = item_value_map::Entity::find()
            .filter(item_value_map::Column::ItemId.eq(item_id))
            .all(&self.database)
            .await?
            .into_iter()
            .map(|mapping| mapping.item_value_id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(item_value::Entity::find()
            .filter(item_value::Column::ItemValueId.is_in(ids))
            .filter(item_value::Column::ValueType.eq(value_type))
            .order_by_asc(item_value::Column::CleanValue)
            .order_by_asc(item_value::Column::ItemValueId)
            .all(&self.database)
            .await?)
    }

    /// Loads base items attached to a normalized value in stable sort order.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn items_for_value(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<Vec<base_item::Model>, ItemValueError> {
        let Some(value) = self.get_normalized(value_type, value).await? else {
            return Ok(Vec::new());
        };
        let ids = item_value_map::Entity::find()
            .filter(item_value_map::Column::ItemValueId.eq(value.item_value_id))
            .all(&self.database)
            .await?
            .into_iter()
            .map(|mapping| mapping.item_id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Id.is_in(ids))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .all(&self.database)
            .await?)
    }
}

async fn upsert_on<C>(
    connection: &C,
    value_type: item_value::ItemValueType,
    value: &str,
) -> Result<item_value::Model, ItemValueError>
where
    C: ConnectionTrait,
{
    let value = validate_value(value)?;
    let clean_value = value.clean_value();
    if clean_value.is_empty() {
        return Err(ItemValueError::InvalidValue);
    }
    let active = item_value::ActiveModel {
        item_value_id: Set(Uuid::new_v4()),
        value_type: Set(value_type),
        value: Set(value.to_owned()),
        clean_value: Set(clean_value),
    };
    Ok(item_value::Entity::insert(active)
        .on_conflict(
            OnConflict::columns([
                item_value::Column::ValueType,
                item_value::Column::CleanValue,
            ])
            .update_column(item_value::Column::CleanValue)
            .to_owned(),
        )
        .exec_with_returning(connection)
        .await?)
}

fn validate_value(value: &str) -> Result<&str, ItemValueError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ItemValueError::InvalidValue)
    } else {
        Ok(value)
    }
}
