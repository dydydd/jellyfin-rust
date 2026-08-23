use std::collections::{HashMap, HashSet};

use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, QueryFilter, QuerySelect, Statement, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, linked_child, playlist, user};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistUserPermission {
    pub user_id: Uuid,
    pub can_edit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistRecord {
    pub item: base_item::Model,
    pub owner_user_id: Option<Uuid>,
    pub open_access: bool,
    pub media_type: Option<String>,
    pub shares: Vec<PlaylistUserPermission>,
}

#[derive(Debug, Error)]
pub enum PlaylistStoreError {
    #[error("playlist was not found")]
    NotFound,
    #[error("playlist user {user_id} was not found")]
    UserNotFound { user_id: Uuid },
    #[error("playlist item {item_id} was not found")]
    ItemNotFound { item_id: Uuid },
    #[error("too many playlist items for PostgreSQL integer sort order")]
    TooManyItems,
    #[error("stored playlist shares are invalid")]
    CorruptShares(#[source] serde_json::Error),
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Clone)]
pub struct PlaylistRepository {
    database: DatabaseConnection,
}

impl PlaylistRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Creates a playlist, permissions, and initial ordered items atomically.
    ///
    /// # Errors
    ///
    /// Returns missing owner/share/item, overflow, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        playlist_id: Uuid,
        name: String,
        parent_id: Uuid,
        owner_user_id: Uuid,
        open_access: bool,
        media_type: Option<String>,
        shares: &[PlaylistUserPermission],
        item_ids: &[Uuid],
    ) -> Result<PlaylistRecord, PlaylistStoreError> {
        let shares = normalize_shares(shares, owner_user_id);
        let item_ids = unique_ids(item_ids);
        let transaction = self.database.begin().await?;
        validate_users(
            &transaction,
            owner_user_id,
            shares.iter().map(|share| share.user_id),
        )
        .await?;
        validate_items(&transaction, item_ids.iter().copied()).await?;

        let item = base_item::Entity::insert(base_item::ActiveModel {
            id: Set(playlist_id),
            item_type: Set("Playlist".to_owned()),
            parent_id: Set(Some(parent_id)),
            name: Set(Some(name.clone())),
            sort_name: Set(Some(name)),
            media_type: Set(media_type.clone()),
            is_folder: Set(true),
            ..Default::default()
        })
        .exec_with_returning(&transaction)
        .await?;
        playlist::Entity::insert(playlist::ActiveModel {
            playlist_id: Set(playlist_id),
            owner_user_id: Set(Some(owner_user_id)),
            open_access: Set(open_access),
            media_type: Set(media_type.clone()),
            shares: Set(serde_json::to_value(&shares).map_err(PlaylistStoreError::CorruptShares)?),
        })
        .exec_without_returning(&transaction)
        .await?;

        if !item_ids.is_empty() {
            let links = item_ids
                .into_iter()
                .enumerate()
                .map(|(index, child_id)| {
                    Ok(linked_child::ActiveModel {
                        parent_id: Set(playlist_id),
                        child_id: Set(child_id),
                        child_type: Set(0),
                        sort_order: Set(Some(
                            i32::try_from(index).map_err(|_| PlaylistStoreError::TooManyItems)?,
                        )),
                    })
                })
                .collect::<Result<Vec<_>, PlaylistStoreError>>()?;
            linked_child::Entity::insert_many(links)
                .exec_without_returning(&transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(PlaylistRecord {
            item,
            owner_user_id: Some(owner_user_id),
            open_access,
            media_type,
            shares,
        })
    }

    /// Loads a playlist and its normalized permission metadata.
    ///
    /// # Errors
    ///
    /// Returns corrupt-share or database errors.
    pub async fn get(
        &self,
        playlist_id: Uuid,
    ) -> Result<Option<PlaylistRecord>, PlaylistStoreError> {
        let Some(metadata) = playlist::Entity::find_by_id(playlist_id)
            .one(&self.database)
            .await?
        else {
            return Ok(None);
        };
        let item = base_item::Entity::find_by_id(playlist_id)
            .one(&self.database)
            .await?
            .ok_or(PlaylistStoreError::NotFound)?;
        let shares =
            serde_json::from_value(metadata.shares).map_err(PlaylistStoreError::CorruptShares)?;
        Ok(Some(PlaylistRecord {
            item,
            owner_user_id: metadata.owner_user_id,
            open_access: metadata.open_access,
            media_type: metadata.media_type,
            shares,
        }))
    }

    /// Replaces selected playlist metadata fields under a `PostgreSQL` row lock.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-share, missing-user, or database errors.
    pub async fn update(
        &self,
        playlist_id: Uuid,
        name: Option<String>,
        open_access: Option<bool>,
        shares: Option<&[PlaylistUserPermission]>,
        item_ids: Option<&[Uuid]>,
    ) -> Result<PlaylistRecord, PlaylistStoreError> {
        let transaction = self.database.begin().await?;
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))",
                [playlist_id.to_string().into()],
            ))
            .await?;
        let metadata = playlist::Entity::find_by_id(playlist_id)
            .one(&transaction)
            .await?
            .ok_or(PlaylistStoreError::NotFound)?;
        if base_item::Entity::find_by_id(playlist_id)
            .one(&transaction)
            .await?
            .is_none()
        {
            return Err(PlaylistStoreError::NotFound);
        }
        let shares = if let Some(shares) = shares {
            let normalized = metadata.owner_user_id.map_or_else(
                || normalize_ownerless_shares(shares),
                |owner| normalize_shares(shares, owner),
            );
            validate_share_users(&transaction, normalized.iter().map(|share| share.user_id))
                .await?;
            normalized
        } else {
            serde_json::from_value(metadata.shares).map_err(PlaylistStoreError::CorruptShares)?
        };
        let item_ids = item_ids.map(unique_ids);
        if let Some(item_ids) = item_ids.as_ref() {
            validate_items(&transaction, item_ids.iter().copied()).await?;
        }
        if let Some(name) = name {
            base_item::Entity::update_many()
                .col_expr(base_item::Column::Name, Expr::value(name.clone()))
                .col_expr(base_item::Column::SortName, Expr::value(name))
                .filter(base_item::Column::Id.eq(playlist_id))
                .exec(&transaction)
                .await?;
        }
        let mut update = playlist::Entity::update_many()
            .col_expr(
                playlist::Column::Shares,
                Expr::value(
                    serde_json::to_value(&shares).map_err(PlaylistStoreError::CorruptShares)?,
                ),
            )
            .filter(playlist::Column::PlaylistId.eq(playlist_id));
        if let Some(open_access) = open_access {
            update = update.col_expr(playlist::Column::OpenAccess, Expr::value(open_access));
        }
        update.exec(&transaction).await?;
        if let Some(item_ids) = item_ids {
            linked_child::Entity::delete_many()
                .filter(linked_child::Column::ParentId.eq(playlist_id))
                .exec(&transaction)
                .await?;
            if !item_ids.is_empty() {
                let links = item_ids
                    .into_iter()
                    .enumerate()
                    .map(|(index, child_id)| {
                        Ok(linked_child::ActiveModel {
                            parent_id: Set(playlist_id),
                            child_id: Set(child_id),
                            child_type: Set(0),
                            sort_order: Set(Some(
                                i32::try_from(index)
                                    .map_err(|_| PlaylistStoreError::TooManyItems)?,
                            )),
                        })
                    })
                    .collect::<Result<Vec<_>, PlaylistStoreError>>()?;
                linked_child::Entity::insert_many(links)
                    .exec_without_returning(&transaction)
                    .await?;
            }
        }
        transaction.commit().await?;
        self.get(playlist_id)
            .await?
            .ok_or(PlaylistStoreError::NotFound)
    }
}

fn normalize_ownerless_shares(shares: &[PlaylistUserPermission]) -> Vec<PlaylistUserPermission> {
    let mut normalized = HashMap::new();
    for share in shares {
        normalized.insert(share.user_id, share.can_edit);
    }
    let mut normalized = normalized
        .into_iter()
        .map(|(user_id, can_edit)| PlaylistUserPermission { user_id, can_edit })
        .collect::<Vec<_>>();
    normalized.sort_unstable_by_key(|share| share.user_id);
    normalized
}

fn normalize_shares(
    shares: &[PlaylistUserPermission],
    owner_user_id: Uuid,
) -> Vec<PlaylistUserPermission> {
    let mut normalized = HashMap::new();
    for share in shares {
        if share.user_id != owner_user_id {
            normalized.insert(share.user_id, share.can_edit);
        }
    }
    let mut normalized = normalized
        .into_iter()
        .map(|(user_id, can_edit)| PlaylistUserPermission { user_id, can_edit })
        .collect::<Vec<_>>();
    normalized.sort_unstable_by_key(|share| share.user_id);
    normalized
}

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

async fn validate_users<C>(
    database: &C,
    owner_user_id: Uuid,
    share_ids: impl Iterator<Item = Uuid>,
) -> Result<(), PlaylistStoreError>
where
    C: ConnectionTrait,
{
    let requested = std::iter::once(owner_user_id)
        .chain(share_ids)
        .collect::<HashSet<_>>();
    let found = user::Entity::find()
        .filter(user::Column::Id.is_in(requested.iter().copied()))
        .all(database)
        .await?
        .into_iter()
        .map(|user| user.id)
        .collect::<HashSet<_>>();
    if let Some(user_id) = requested.into_iter().find(|id| !found.contains(id)) {
        return Err(PlaylistStoreError::UserNotFound { user_id });
    }
    Ok(())
}

async fn validate_share_users<C>(
    database: &C,
    share_ids: impl Iterator<Item = Uuid>,
) -> Result<(), PlaylistStoreError>
where
    C: ConnectionTrait,
{
    let requested = share_ids.collect::<HashSet<_>>();
    if requested.is_empty() {
        return Ok(());
    }
    let found = user::Entity::find()
        .select_only()
        .column(user::Column::Id)
        .filter(user::Column::Id.is_in(requested.iter().copied()))
        .into_tuple::<Uuid>()
        .all(database)
        .await?
        .into_iter()
        .collect::<HashSet<_>>();
    if let Some(user_id) = requested.into_iter().find(|id| !found.contains(id)) {
        return Err(PlaylistStoreError::UserNotFound { user_id });
    }
    Ok(())
}

async fn validate_items<C>(
    database: &C,
    item_ids: impl Iterator<Item = Uuid>,
) -> Result<(), PlaylistStoreError>
where
    C: ConnectionTrait,
{
    let requested = item_ids.collect::<HashSet<_>>();
    if requested.is_empty() {
        return Ok(());
    }
    let found = base_item::Entity::find()
        .filter(base_item::Column::Id.is_in(requested.iter().copied()))
        .all(database)
        .await?
        .into_iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    if let Some(item_id) = requested.into_iter().find(|id| !found.contains(id)) {
        return Err(PlaylistStoreError::ItemNotFound { item_id });
    }
    Ok(())
}
