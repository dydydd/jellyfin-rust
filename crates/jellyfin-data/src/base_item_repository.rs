use std::collections::HashMap;

use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    DbBackend, DbErr, DeleteResult, EntityTrait, QueryFilter, QueryOrder, SqlErr, Statement,
    TransactionTrait,
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{ancestor_id, base_item};

const HIERARCHY_ADVISORY_LOCK_KEY: i64 = 0x4241_5345_4954_454d;
pub const USER_ROOT_FOLDER_ID: Uuid = Uuid::from_u128(2);

/// Values accepted when creating a persisted Jellyfin base item.
#[derive(Debug, Clone, PartialEq)]
pub struct NewBaseItem {
    pub id: Uuid,
    pub item_type: String,
    pub data: Option<Value>,
    pub path: Option<String>,
    pub parent_id: Option<Uuid>,
    pub name: Option<String>,
    pub sort_name: Option<String>,
    pub media_type: Option<String>,
    pub overview: Option<String>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
    pub runtime_ticks: Option<i64>,
    pub is_folder: bool,
    pub is_virtual_item: bool,
    pub presentation_unique_key: Option<String>,
    pub series_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    pub series_presentation_unique_key: Option<String>,
}

impl NewBaseItem {
    #[must_use]
    pub fn new(id: Uuid, item_type: impl Into<String>) -> Self {
        Self {
            id,
            item_type: item_type.into(),
            data: None,
            path: None,
            parent_id: None,
            name: None,
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
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BaseItemHierarchyEntry {
    pub item: base_item::Model,
    pub depth: i32,
}

#[derive(Debug, Error)]
pub enum BaseItemError {
    #[error("base item type cannot be empty")]
    InvalidItemType,
    #[error("base item was not found")]
    NotFound,
    #[error("base item parent was not found")]
    ParentNotFound,
    #[error("base item hierarchy cannot contain a cycle")]
    HierarchyCycle,
    #[error("base item was changed by another writer")]
    StaleVersion,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed item metadata and hierarchy persistence.
#[derive(Clone)]
pub struct BaseItemRepository {
    database: DatabaseConnection,
}

impl BaseItemRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Returns the single persisted user-library root, creating it when the
    /// database has not been initialized yet.
    ///
    /// The hierarchy advisory lock makes concurrent server startups converge
    /// on one row. The reserved identifier also makes initialization
    /// idempotent across restarts.
    ///
    /// # Errors
    ///
    /// Returns a database error when the root cannot be loaded or created.
    pub async fn ensure_user_root(&self) -> Result<base_item::Model, BaseItemError> {
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;

        if let Some(root) = base_item::Entity::find()
            .filter(base_item::Column::ItemType.eq("UserRootFolder"))
            .filter(base_item::Column::ParentId.is_null())
            .order_by_asc(base_item::Column::DateCreated)
            .order_by_asc(base_item::Column::Id)
            .one(&transaction)
            .await?
        {
            transaction.commit().await?;
            return Ok(root);
        }

        let root = base_item::ActiveModel {
            id: Set(USER_ROOT_FOLDER_ID),
            item_type: Set("UserRootFolder".to_owned()),
            name: Set(Some("Root".to_owned())),
            sort_name: Set(Some("Root".to_owned())),
            is_folder: Set(true),
            ..Default::default()
        };
        let root = base_item::Entity::insert(root)
            .exec_with_returning(&transaction)
            .await
            .map_err(map_database_error)?;
        transaction.commit().await?;
        Ok(root)
    }

    /// Inserts an item and atomically maintains its closure-table rows.
    ///
    /// # Errors
    ///
    /// Returns a validation, hierarchy, or database error.
    pub async fn create(&self, item: NewBaseItem) -> Result<base_item::Model, BaseItemError> {
        validate_item_type(&item.item_type)?;
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        validate_parent(&transaction, item.id, item.parent_id).await?;

        let model = base_item::ActiveModel {
            id: Set(item.id),
            item_type: Set(item.item_type),
            data: Set(item.data),
            path: Set(item.path),
            parent_id: Set(item.parent_id),
            name: Set(item.name),
            sort_name: Set(item.sort_name),
            media_type: Set(item.media_type),
            overview: Set(item.overview),
            index_number: Set(item.index_number),
            parent_index_number: Set(item.parent_index_number),
            production_year: Set(item.production_year),
            runtime_ticks: Set(item.runtime_ticks),
            is_folder: Set(item.is_folder),
            is_virtual_item: Set(item.is_virtual_item),
            presentation_unique_key: Set(item.presentation_unique_key),
            series_id: Set(item.series_id),
            season_id: Set(item.season_id),
            series_presentation_unique_key: Set(item.series_presentation_unique_key),
            ..Default::default()
        };
        let inserted = base_item::Entity::insert(model)
            .exec_with_returning(&transaction)
            .await
            .map_err(map_database_error)?;
        transaction.commit().await?;
        Ok(inserted)
    }

    /// Loads an item by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get(&self, id: Uuid) -> Result<Option<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find_by_id(id)
            .one(&self.database)
            .await?)
    }

    /// Reports whether an item identifier is present.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn exists(&self, id: Uuid) -> Result<bool, BaseItemError> {
        Ok(self.get(id).await?.is_some())
    }

    /// Uses the `PostgreSQL` partial hash index to test an exact item path.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn exists_by_path(&self, path: &str) -> Result<bool, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Path.eq(path))
            .one(&self.database)
            .await?
            .is_some())
    }

    /// Replaces mutable fields using `row_version` as an optimistic lock.
    ///
    /// # Errors
    ///
    /// Returns `StaleVersion` when another writer already updated the row.
    pub async fn update(&self, item: base_item::Model) -> Result<base_item::Model, BaseItemError> {
        validate_item_type(&item.item_type)?;
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        let current = base_item::Entity::find_by_id(item.id)
            .one(&transaction)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if current.row_version != item.row_version {
            return Err(BaseItemError::StaleVersion);
        }
        validate_parent(&transaction, item.id, item.parent_id).await?;

        let changes = base_item::ActiveModel {
            item_type: Set(item.item_type),
            data: Set(item.data),
            path: Set(item.path),
            parent_id: Set(item.parent_id),
            name: Set(item.name),
            sort_name: Set(item.sort_name),
            media_type: Set(item.media_type),
            overview: Set(item.overview),
            index_number: Set(item.index_number),
            parent_index_number: Set(item.parent_index_number),
            production_year: Set(item.production_year),
            runtime_ticks: Set(item.runtime_ticks),
            is_folder: Set(item.is_folder),
            is_virtual_item: Set(item.is_virtual_item),
            presentation_unique_key: Set(item.presentation_unique_key),
            series_id: Set(item.series_id),
            season_id: Set(item.season_id),
            series_presentation_unique_key: Set(item.series_presentation_unique_key),
            ..Default::default()
        };
        let result = base_item::Entity::update_many()
            .set(changes)
            .filter(base_item::Column::Id.eq(item.id))
            .filter(base_item::Column::RowVersion.eq(item.row_version))
            .exec(&transaction)
            .await
            .map_err(map_database_error)?;
        if result.rows_affected == 0 {
            return Err(BaseItemError::StaleVersion);
        }
        let updated = base_item::Entity::find_by_id(item.id)
            .one(&transaction)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        transaction.commit().await?;
        Ok(updated)
    }

    /// Moves an item while preserving optimistic-lock semantics.
    ///
    /// # Errors
    ///
    /// Returns hierarchy, stale-version, not-found, or database errors.
    pub async fn move_item(
        &self,
        id: Uuid,
        parent_id: Option<Uuid>,
        expected_row_version: i64,
    ) -> Result<base_item::Model, BaseItemError> {
        let mut item = self.get(id).await?.ok_or(BaseItemError::NotFound)?;
        if item.row_version != expected_row_version {
            return Err(BaseItemError::StaleVersion);
        }
        item.parent_id = parent_id;
        self.update(item).await
    }

    /// Deletes an item; the database cascades through the complete subtree.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete(&self, id: Uuid) -> Result<bool, BaseItemError> {
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        if base_item::Entity::find_by_id(id)
            .one(&transaction)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let DeleteResult { rows_affected } = base_item::Entity::delete_by_id(id)
            .exec(&transaction)
            .await?;
        transaction.commit().await?;
        Ok(rows_affected == 1)
    }

    /// Loads the direct parent of an item.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the item itself is absent.
    pub async fn parent(&self, id: Uuid) -> Result<Option<base_item::Model>, BaseItemError> {
        let item = self.get(id).await?.ok_or(BaseItemError::NotFound)?;
        match item.parent_id {
            Some(parent_id) => self.get(parent_id).await,
            None => Ok(None),
        }
    }

    /// Loads direct children in stable sort-name and identifier order.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn children(&self, id: Uuid) -> Result<Vec<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::ParentId.eq(id))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .all(&self.database)
            .await?)
    }

    /// Loads all ancestors nearest-first with their closure-table depths.
    ///
    /// # Errors
    ///
    /// Returns a database error when either query fails.
    pub async fn ancestors(&self, id: Uuid) -> Result<Vec<BaseItemHierarchyEntry>, BaseItemError> {
        let closure = ancestor_id::Entity::find()
            .filter(ancestor_id::Column::ItemId.eq(id))
            .order_by_asc(ancestor_id::Column::Depth)
            .all(&self.database)
            .await?;
        hierarchy_entries(closure, false, &self.database).await
    }

    /// Loads all descendants in stable depth and identifier order.
    ///
    /// # Errors
    ///
    /// Returns a database error when either query fails.
    pub async fn descendants(
        &self,
        id: Uuid,
    ) -> Result<Vec<BaseItemHierarchyEntry>, BaseItemError> {
        let closure = ancestor_id::Entity::find()
            .filter(ancestor_id::Column::ParentItemId.eq(id))
            .order_by_asc(ancestor_id::Column::Depth)
            .order_by_asc(ancestor_id::Column::ItemId)
            .all(&self.database)
            .await?;
        hierarchy_entries(closure, true, &self.database).await
    }
}

async fn acquire_hierarchy_lock(transaction: &DatabaseTransaction) -> Result<(), BaseItemError> {
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [HIERARCHY_ADVISORY_LOCK_KEY.into()],
        ))
        .await?;
    Ok(())
}

async fn validate_parent(
    transaction: &DatabaseTransaction,
    item_id: Uuid,
    parent_id: Option<Uuid>,
) -> Result<(), BaseItemError> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    if parent_id == item_id {
        return Err(BaseItemError::HierarchyCycle);
    }
    if base_item::Entity::find_by_id(parent_id)
        .one(transaction)
        .await?
        .is_none()
    {
        return Err(BaseItemError::ParentNotFound);
    }
    if ancestor_id::Entity::find_by_id((parent_id, item_id))
        .one(transaction)
        .await?
        .is_some()
    {
        return Err(BaseItemError::HierarchyCycle);
    }
    Ok(())
}

async fn hierarchy_entries(
    closure: Vec<ancestor_id::Model>,
    use_item_id: bool,
    database: &DatabaseConnection,
) -> Result<Vec<BaseItemHierarchyEntry>, BaseItemError> {
    let ids: Vec<Uuid> = closure
        .iter()
        .map(|row| {
            if use_item_id {
                row.item_id
            } else {
                row.parent_item_id
            }
        })
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let items = base_item::Entity::find()
        .filter(base_item::Column::Id.is_in(ids))
        .all(database)
        .await?;
    let mut by_id: HashMap<Uuid, base_item::Model> =
        items.into_iter().map(|item| (item.id, item)).collect();
    Ok(closure
        .into_iter()
        .filter_map(|row| {
            let id = if use_item_id {
                row.item_id
            } else {
                row.parent_item_id
            };
            by_id.remove(&id).map(|item| BaseItemHierarchyEntry {
                item,
                depth: row.depth,
            })
        })
        .collect())
}

fn validate_item_type(item_type: &str) -> Result<(), BaseItemError> {
    if item_type.trim().is_empty() {
        Err(BaseItemError::InvalidItemType)
    } else {
        Ok(())
    }
}

fn map_database_error(error: DbErr) -> BaseItemError {
    let message = error.to_string();
    if message.contains("base_items_hierarchy_acyclic")
        || message.contains("base_items_parent_not_self")
    {
        BaseItemError::HierarchyCycle
    } else if matches!(
        error.sql_err(),
        Some(SqlErr::ForeignKeyConstraintViolation(_))
    ) && message.contains("base_items_parent_id_fkey")
    {
        BaseItemError::ParentNotFound
    } else {
        BaseItemError::Database(error)
    }
}
