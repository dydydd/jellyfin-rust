use std::collections::HashSet;

use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, Statement, TransactionTrait,
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, linked_child};

const HIERARCHY_ADVISORY_LOCK_KEY: i64 = 0x4241_5345_4954_454d;

#[derive(Debug, Error)]
pub enum CollectionStoreError {
    #[error("collection parent was not found")]
    ParentNotFound,
    #[error("collection child {child_id} was not found")]
    ChildNotFound { child_id: Uuid },
    #[error("a collection cannot contain itself")]
    SelfLink,
    #[error("too many collection children for PostgreSQL integer sort order")]
    TooManyChildren,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// Atomic persistence used when creating a collection and its initial links.
#[derive(Clone)]
pub struct CollectionRepository {
    database: DatabaseConnection,
}

impl CollectionRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Creates a `BoxSet` and all requested manual children in one transaction.
    ///
    /// The hierarchy advisory lock is shared with base-item writers, while the
    /// initial members use one `PostgreSQL` bulk insert in request order.
    ///
    /// # Errors
    ///
    /// Returns a missing parent/child, self-link, overflow, or database error.
    pub async fn create(
        &self,
        id: Uuid,
        name: Option<String>,
        parent_id: Option<Uuid>,
        is_locked: bool,
        child_ids: &[Uuid],
    ) -> Result<base_item::Model, CollectionStoreError> {
        let child_ids = unique_ids(child_ids);
        if child_ids.contains(&id) {
            return Err(CollectionStoreError::SelfLink);
        }

        let transaction = self.database.begin().await?;
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT pg_advisory_xact_lock($1)",
                [HIERARCHY_ADVISORY_LOCK_KEY.into()],
            ))
            .await?;
        if let Some(parent_id) = parent_id
            && base_item::Entity::find_by_id(parent_id)
                .one(&transaction)
                .await?
                .is_none()
        {
            return Err(CollectionStoreError::ParentNotFound);
        }
        validate_children(&transaction, &child_ids).await?;

        let collection = base_item::Entity::insert(base_item::ActiveModel {
            id: Set(id),
            item_type: Set("BoxSet".to_owned()),
            data: Set(Some(json!({ "IsLocked": is_locked }))),
            parent_id: Set(parent_id),
            name: Set(name.clone()),
            sort_name: Set(name),
            is_folder: Set(true),
            ..Default::default()
        })
        .exec_with_returning(&transaction)
        .await?;

        if !child_ids.is_empty() {
            let inserts = child_ids
                .into_iter()
                .enumerate()
                .map(|(sort_order, child_id)| {
                    Ok(linked_child::ActiveModel {
                        parent_id: Set(id),
                        child_id: Set(child_id),
                        child_type: Set(0),
                        sort_order: Set(Some(
                            i32::try_from(sort_order)
                                .map_err(|_| CollectionStoreError::TooManyChildren)?,
                        )),
                    })
                })
                .collect::<Result<Vec<_>, CollectionStoreError>>()?;
            linked_child::Entity::insert_many(inserts)
                .exec_without_returning(&transaction)
                .await?;
        }

        transaction.commit().await?;
        Ok(collection)
    }
}

async fn validate_children<C>(database: &C, child_ids: &[Uuid]) -> Result<(), CollectionStoreError>
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
        return Err(CollectionStoreError::ChildNotFound {
            child_id: *child_id,
        });
    }
    Ok(())
}

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}
