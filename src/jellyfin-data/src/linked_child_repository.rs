use std::collections::HashSet;

use sea_orm::{
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
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
    database: crate::SharedDatabase,
}

impl LinkedChildRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
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
            .all(self.database.as_ref())
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
        self.add_manual_at(parent_id, child_ids, None).await
    }

    /// Adds unique manual links at a requested zero-based position.
    ///
    /// The parent row lock and one `PostgreSQL` upsert keep insertion and
    /// renumbering atomic when several clients edit the same playlist.
    ///
    /// # Errors
    ///
    /// Returns missing-item, self-link, overflow, or database errors.
    pub async fn add_manual_at(
        &self,
        parent_id: Uuid,
        child_ids: &[Uuid],
        position: Option<i32>,
    ) -> Result<Vec<LinkedChild>, LinkedChildStoreError> {
        let child_ids = unique_ids(child_ids);
        if child_ids.contains(&parent_id) {
            return Err(LinkedChildStoreError::SelfLink);
        }
        let transaction = self.database.begin().await?;
        lock_parent(&transaction, parent_id).await?;
        validate_children(&transaction, &child_ids).await?;

        let existing = list_models_with(&transaction, parent_id).await?;
        let existing_ids = existing
            .iter()
            .map(|link| link.child_id)
            .collect::<HashSet<_>>();
        let new_ids = child_ids
            .into_iter()
            .filter(|child_id| !existing_ids.contains(child_id))
            .collect::<Vec<_>>();
        if !new_ids.is_empty() {
            let insert_at = position.map_or(existing.len(), |position| {
                usize::try_from(position.max(0))
                    .unwrap_or(usize::MAX)
                    .min(existing.len())
            });
            let mut ordered = existing;
            ordered.splice(
                insert_at..insert_at,
                new_ids.into_iter().map(|child_id| linked_child::Model {
                    parent_id,
                    child_id,
                    child_type: LinkedChildType::Manual as i16,
                    sort_order: None,
                }),
            );
            let rows = ordered_rows(parent_id, ordered)?;
            linked_child::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([
                        linked_child::Column::ParentId,
                        linked_child::Column::ChildId,
                    ])
                    .update_column(linked_child::Column::SortOrder)
                    .to_owned(),
                )
                .exec_without_returning(&transaction)
                .await?;
        }
        let links = list_with(&transaction, parent_id).await?;
        transaction.commit().await?;
        Ok(links)
    }

    /// Moves one linked child to a clamped zero-based position.
    ///
    /// # Errors
    ///
    /// Returns a missing-parent, overflow, or database error.
    pub async fn move_to(
        &self,
        parent_id: Uuid,
        child_id: Uuid,
        new_index: usize,
    ) -> Result<Vec<LinkedChild>, LinkedChildStoreError> {
        let transaction = self.database.begin().await?;
        lock_parent(&transaction, parent_id).await?;
        let mut ordered = list_models_with(&transaction, parent_id).await?;
        let Some(old_index) = ordered.iter().position(|link| link.child_id == child_id) else {
            let links = ordered
                .into_iter()
                .map(LinkedChild::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            transaction.commit().await?;
            return Ok(links);
        };
        if old_index != new_index {
            let item = ordered.remove(old_index);
            let insert_at = new_index.min(ordered.len());
            ordered.insert(insert_at, item);
            let rows = ordered_rows(parent_id, ordered)?;
            linked_child::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([
                        linked_child::Column::ParentId,
                        linked_child::Column::ChildId,
                    ])
                    .update_column(linked_child::Column::SortOrder)
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
        self.remove_inner(parent_id, child_ids, false).await
    }

    /// Removes links and rewrites the remaining entries to consecutive positions.
    ///
    /// # Errors
    ///
    /// Returns a missing-parent, overflow, or database error.
    pub async fn remove_compact(
        &self,
        parent_id: Uuid,
        child_ids: &[Uuid],
    ) -> Result<Vec<LinkedChild>, LinkedChildStoreError> {
        self.remove_inner(parent_id, child_ids, true).await
    }

    async fn remove_inner(
        &self,
        parent_id: Uuid,
        child_ids: &[Uuid],
        compact: bool,
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
            let remaining = if compact {
                list_models_with(&transaction, parent_id).await?
            } else {
                Vec::new()
            };
            if compact && !remaining.is_empty() {
                linked_child::Entity::insert_many(ordered_rows(parent_id, remaining)?)
                    .on_conflict(
                        OnConflict::columns([
                            linked_child::Column::ParentId,
                            linked_child::Column::ChildId,
                        ])
                        .update_column(linked_child::Column::SortOrder)
                        .to_owned(),
                    )
                    .exec_without_returning(&transaction)
                    .await?;
            }
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

async fn list_models_with<C>(
    database: &C,
    parent_id: Uuid,
) -> Result<Vec<linked_child::Model>, LinkedChildStoreError>
where
    C: ConnectionTrait,
{
    Ok(linked_child::Entity::find()
        .filter(linked_child::Column::ParentId.eq(parent_id))
        .order_by_with_nulls(
            linked_child::Column::SortOrder,
            Order::Asc,
            NullOrdering::Last,
        )
        .order_by_asc(linked_child::Column::ChildId)
        .all(database)
        .await?)
}

fn ordered_rows(
    parent_id: Uuid,
    ordered: Vec<linked_child::Model>,
) -> Result<Vec<linked_child::ActiveModel>, LinkedChildStoreError> {
    ordered
        .into_iter()
        .enumerate()
        .map(|(index, link)| {
            Ok(linked_child::ActiveModel {
                parent_id: Set(parent_id),
                child_id: Set(link.child_id),
                child_type: Set(link.child_type),
                sort_order: Set(Some(
                    i32::try_from(index).map_err(|_| LinkedChildStoreError::SortOrderOverflow)?,
                )),
            })
        })
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
