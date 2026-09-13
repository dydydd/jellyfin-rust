use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DbBackend, DbErr, EntityTrait, Statement, TransactionTrait};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::emby_item_access;

/// Emby's protocol-private persisted share level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i16)]
pub enum EmbyItemAccessLevel {
    Read = 1,
    Write = 2,
    Manage = 3,
    ManageDelete = 4,
}

#[derive(Debug, Error)]
pub enum EmbyItemAccessStoreError {
    #[error("one or more users were not found")]
    UserNotFound,
    #[error("one or more base items were not found")]
    ItemNotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL storage for Emby's `(user, item) -> UserItemShareLevel` map.
#[derive(Clone)]
pub struct EmbyItemAccessRepository {
    database: crate::SharedDatabase,
}

impl EmbyItemAccessRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Replaces the explicit access level for every requested user/item pair.
    ///
    /// Inputs are deduplicated, every referenced owner is locked and validated
    /// before the first write, and the Cartesian-product mutation commits as
    /// one transaction. `None` is Emby's `UserItemShareLevel.None` and removes
    /// the explicit assignments instead of storing a redundant zero row.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-user/item error or a PostgreSQL error. No pair
    /// is changed on error.
    pub async fn replace(
        &self,
        user_ids: &[Uuid],
        item_ids: &[Uuid],
        access: Option<EmbyItemAccessLevel>,
    ) -> Result<(), EmbyItemAccessStoreError> {
        let user_ids = unique_ids(user_ids);
        let item_ids = unique_ids(item_ids);
        let transaction = self.database.begin().await?;

        let users = lock_existing_ids(&transaction, "jellyfin.users", &json_ids(&user_ids)).await?;
        if users != user_ids.len() {
            return Err(EmbyItemAccessStoreError::UserNotFound);
        }
        let items =
            lock_existing_ids(&transaction, "jellyfin.base_items", &json_ids(&item_ids)).await?;
        if items != item_ids.len() {
            return Err(EmbyItemAccessStoreError::ItemNotFound);
        }

        if !user_ids.is_empty() && !item_ids.is_empty() {
            let users = Value::Array(
                user_ids
                    .iter()
                    .map(|id| Value::String(id.to_string()))
                    .collect(),
            );
            let items = Value::Array(
                item_ids
                    .iter()
                    .map(|id| Value::String(id.to_string()))
                    .collect(),
            );
            let (sql, values) = if let Some(access) = access {
                (
                    r"
                    WITH requested_users AS (
                        SELECT value::uuid AS user_id
                        FROM jsonb_array_elements_text($1::jsonb)
                    ), requested_items AS (
                        SELECT value::uuid AS item_id
                        FROM jsonb_array_elements_text($2::jsonb)
                    )
                    INSERT INTO jellyfin.emby_item_access (
                        user_id, item_id, access_level
                    )
                    SELECT users.user_id, items.item_id, $3
                    FROM requested_users AS users
                    CROSS JOIN requested_items AS items
                    ON CONFLICT (user_id, item_id) DO UPDATE
                    SET access_level = EXCLUDED.access_level,
                        updated_at = clock_timestamp()
                    ",
                    vec![users.into(), items.into(), (access as i16).into()],
                )
            } else {
                (
                    r"
                    WITH requested_users AS (
                        SELECT value::uuid AS user_id
                        FROM jsonb_array_elements_text($1::jsonb)
                    ), requested_items AS (
                        SELECT value::uuid AS item_id
                        FROM jsonb_array_elements_text($2::jsonb)
                    )
                    DELETE FROM jellyfin.emby_item_access AS access
                    USING requested_users AS users, requested_items AS items
                    WHERE access.user_id = users.user_id
                      AND access.item_id = items.item_id
                    ",
                    vec![users.into(), items.into()],
                )
            };
            transaction
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    values,
                ))
                .await?;
        }

        transaction.commit().await?;
        Ok(())
    }

    /// Loads one explicit Emby access assignment.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get(
        &self,
        user_id: Uuid,
        item_id: Uuid,
    ) -> Result<Option<emby_item_access::Model>, DbErr> {
        emby_item_access::Entity::find_by_id((user_id, item_id))
            .one(self.database.as_ref())
            .await
    }
}

async fn lock_existing_ids<C: ConnectionTrait>(
    database: &C,
    table: &str,
    ids: &Value,
) -> Result<usize, DbErr> {
    if ids.as_array().is_none_or(Vec::is_empty) {
        return Ok(0);
    }
    let sql = format!(
        "SELECT owner.id FROM {table} AS owner \
         JOIN (SELECT value::uuid AS id FROM jsonb_array_elements_text($1::jsonb)) AS requested \
           ON requested.id = owner.id \
         FOR KEY SHARE OF owner"
    );
    Ok(database
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [ids.clone().into()],
        ))
        .await?
        .len())
}

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::with_capacity(ids.len());
    let mut unique = ids
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect::<Vec<_>>();
    // Lock rows in a stable order so concurrent overlapping Cartesian
    // mutations cannot deadlock merely because clients order ids differently.
    unique.sort_unstable();
    unique
}

fn json_ids(ids: &[Uuid]) -> Value {
    Value::Array(ids.iter().map(|id| Value::String(id.to_string())).collect())
}
