use std::collections::HashSet;

use sea_orm::{
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, TransactionTrait,
    sea_query::{NullOrdering, OnConflict, Order},
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, linked_child};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i16)]
pub enum LinkedChildType {
    Manual = 0,
    Shortcut = 1,
    LocalAlternateVersion = 2,
    LinkedAlternateVersion = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkedChild {
    pub parent_id: Uuid,
    pub child_id: Uuid,
    pub child_type: LinkedChildType,
    pub sort_order: Option<i32>,
}

#[derive(Debug, Error)]
pub enum LinkedChildStoreError {
    #[error("parent item {parent_id} was not found")]
    ParentNotFound { parent_id: Uuid },
    #[error("child item {child_id} was not found")]
    ChildNotFound { child_id: Uuid },
    #[error("an item cannot link to itself")]
    SelfLink,
    #[error("linked-child sort order overflowed PostgreSQL integer range")]
    SortOrderOverflow,
    #[error("stored linked-child type {0} is invalid")]
    CorruptChildType(i16),
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct LinkedChildRepository {
    database: DatabaseConnection,
}

impl LinkedChildRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Lists one parent's links in their persisted presentation order.
    ///
    /// # Errors
    ///
    /// Returns a database or corrupt-row error.
    pub async fn list(&self, parent_id: Uuid) -> Result<Vec<LinkedChild>, LinkedChildStoreError> {
        linked_child::Entity::find()
            .filter(linked_child::Column::ParentId.eq(parent_id))
            .order_by_with_nulls(
                linked_child::Column::SortOrder,
                Order::Asc,
                NullOrdering::Last,
            )
            .order_by_asc(linked_child::Column::ChildId)
            .all(&self.database)
            .await?
            .into_iter()
            .map(LinkedChild::try_from)
            .collect()
    }

    /// Adds manual links, preserving input order and existing positions.
    ///
    /// A lock on the parent item serializes competing append operations. The
    /// bulk insert uses `PostgreSQL`'s composite-key conflict handler for stable
    /// idempotency without a per-child round trip.
    ///
    /// # Errors
    ///
    /// Returns missing-item, self-link, overflow, or database errors.
    pub async fn add_manual(
        &self,
        parent_id: Uuid,
        child_ids: &[Uuid],
    ) -> Result<Vec<LinkedChild>, LinkedChildStoreError> {
        let child_ids = unique_ids(child_ids);
        if child_ids.contains(&parent_id) {
            return Err(LinkedChildStoreError::SelfLink);
        }
        let transaction = self.database.begin().await?;
        lock_parent(&transaction, parent_id).await?;
        validate_children(&transaction, &child_ids).await?;

        let existing = linked_child::Entity::find()
            .filter(linked_child::Column::ParentId.eq(parent_id))
            .all(&transaction)
            .await?;
        let existing_ids = existing
            .iter()
            .map(|link| link.child_id)
            .collect::<HashSet<_>>();
        let first_order = existing
            .iter()
            .filter_map(|link| link.sort_order)
            .max()
            .unwrap_or(-1)
            .checked_add(1)
            .ok_or(LinkedChildStoreError::SortOrderOverflow)?;
        let mut inserts = Vec::new();
        for child_id in child_ids {
            if existing_ids.contains(&child_id) {
                continue;
            }
            let offset = i32::try_from(inserts.len())
                .map_err(|_| LinkedChildStoreError::SortOrderOverflow)?;
            let sort_order = first_order
                .checked_add(offset)
                .ok_or(LinkedChildStoreError::SortOrderOverflow)?;
            inserts.push(linked_child::ActiveModel {
                parent_id: Set(parent_id),
                child_id: Set(child_id),
                child_type: Set(LinkedChildType::Manual as i16),
                sort_order: Set(Some(sort_order)),
            });
        }
        if !inserts.is_empty() {
            linked_child::Entity::insert_many(inserts)
                .on_conflict(
                    OnConflict::columns([
                        linked_child::Column::ParentId,
                        linked_child::Column::ChildId,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .exec_without_returning(&transaction)
                .await?;
        }
        let links = list_with(&transaction, parent_id).await?;
        transaction.commit().await?;
        Ok(links)
    }

    /// Removes matching links while retaining the order of remaining entries.
    ///
    /// # Errors
    ///
    /// Returns a missing-parent or database error.
    pub async fn remove(
        &self,
        parent_id: Uuid,
        child_ids: &[Uuid],
    ) -> Result<Vec<LinkedChild>, LinkedChildStoreError> {
        let child_ids = unique_ids(child_ids);
        let transaction = self.database.begin().await?;
        lock_parent(&transaction, parent_id).await?;
        if !child_ids.is_empty() {
            linked_child::Entity::delete_many()
                .filter(linked_child::Column::ParentId.eq(parent_id))
                .filter(linked_child::Column::ChildId.is_in(child_ids))
                .exec(&transaction)
                .await?;
        }
        let links = list_with(&transaction, parent_id).await?;
        transaction.commit().await?;
        Ok(links)
    }
}

async fn lock_parent<C>(database: &C, parent_id: Uuid) -> Result<(), LinkedChildStoreError>
where
    C: ConnectionTrait,
{
    base_item::Entity::find_by_id(parent_id)
        .lock_exclusive()
        .one(database)
        .await?
        .ok_or(LinkedChildStoreError::ParentNotFound { parent_id })?;
    Ok(())
}

async fn validate_children<C>(database: &C, child_ids: &[Uuid]) -> Result<(), LinkedChildStoreError>
where
    C: ConnectionTrait,
{
    if child_ids.is_empty() {
        return Ok(());
    }
    let found = base_item::Entity::find()
        .filter(base_item::Column::Id.is_in(child_ids.iter().copied()))
        .all(database)
        .await?
        .into_iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    if let Some(child_id) = child_ids.iter().find(|child_id| !found.contains(child_id)) {
        return Err(LinkedChildStoreError::ChildNotFound {
            child_id: *child_id,
        });
    }
    Ok(())
}

async fn list_with<C>(
    database: &C,
    parent_id: Uuid,
) -> Result<Vec<LinkedChild>, LinkedChildStoreError>
where
    C: ConnectionTrait,
{
    linked_child::Entity::find()
        .filter(linked_child::Column::ParentId.eq(parent_id))
        .order_by_with_nulls(
            linked_child::Column::SortOrder,
            Order::Asc,
            NullOrdering::Last,
        )
        .order_by_asc(linked_child::Column::ChildId)
        .all(database)
        .await?
        .into_iter()
        .map(LinkedChild::try_from)
        .collect()
}

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

impl TryFrom<linked_child::Model> for LinkedChild {
    type Error = LinkedChildStoreError;

    fn try_from(row: linked_child::Model) -> Result<Self, Self::Error> {
        let child_type = match row.child_type {
            0 => LinkedChildType::Manual,
            1 => LinkedChildType::Shortcut,
            2 => LinkedChildType::LocalAlternateVersion,
            3 => LinkedChildType::LinkedAlternateVersion,
            value => return Err(LinkedChildStoreError::CorruptChildType(value)),
        };
        Ok(Self {
            parent_id: row.parent_id,
            child_id: row.child_id,
            child_type,
            sort_order: row.sort_order,
        })
    }
}
