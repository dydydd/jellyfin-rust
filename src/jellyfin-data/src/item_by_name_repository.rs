use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, Statement, TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{NewItemByNameEntity, entities::base_item, item_types::expand_item_type_aliases};

#[derive(Debug, Error)]
pub enum ItemByNameStoreError {
    #[error("unsupported item-by-name type {0}")]
    UnsupportedType(String),
    #[error("item-by-name id resolved to an incompatible item type")]
    IncompatibleExistingItem,
    #[error("item-by-name insert did not produce a persisted row")]
    MissingInsertedItem,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// Exact persistence operations used by Jellyfin item-by-name resolvers.
#[derive(Clone)]
pub struct ItemByNameRepository {
    database: crate::SharedDatabase,
}

impl ItemByNameRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Loads the deterministic item only when its persisted type matches the
    /// canonical or legacy CLR name.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get(
        &self,
        id: Uuid,
        item_type: &str,
    ) -> Result<Option<base_item::Model>, ItemByNameStoreError> {
        let item_types = supported_item_types(item_type)?;
        Ok(base_item::Entity::find_by_id(id)
            .filter(base_item::Column::ItemType.is_in(item_types))
            .one(self.database.as_ref())
            .await?)
    }

    /// Resolves one persisted entity using Jellyfin's normalized `Name`
    /// predicate. The caller controls the official slug-substitution order.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get_by_name(
        &self,
        item_type: &str,
        name: &str,
    ) -> Result<Option<base_item::Model>, ItemByNameStoreError> {
        let item_types = supported_item_types(item_type)?;
        Ok(base_item::Entity::find()
            .filter(base_item::Column::ItemType.is_in(item_types))
            .filter(base_item::Column::CleanName.eq(name.clean_value()))
            .order_by_asc(base_item::Column::Id)
            .one(self.database.as_ref())
            .await?)
    }

    /// Resolves the exact raw-name `MusicArtist` preferred by official Jellyfin.
    /// Physical library artists win over accessed-by-name rows, with the id as
    /// a deterministic tie breaker.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get_music_artist_by_raw_name(
        &self,
        name: &str,
    ) -> Result<Option<base_item::Model>, ItemByNameStoreError> {
        let item_types = supported_item_types("MusicArtist")?;
        let mut values = vec![name.to_owned().into()];
        let placeholders = (2..=item_types.len() + 1)
            .map(|index| format!("${index}::text"))
            .collect::<Vec<_>>()
            .join(", ");
        values.extend(item_types.into_iter().map(Into::into));
        Ok(
            base_item::Model::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "SELECT item.* FROM jellyfin.base_items AS item \
                 WHERE item.name = $1::text \
                   AND item.item_type IN ({placeholders}) \
                 ORDER BY CASE WHEN item.parent_id IS NULL THEN 1 ELSE 0 END, item.id \
                 LIMIT 1"
                ),
                values,
            ))
            .one(self.database.as_ref())
            .await?,
        )
    }

    /// Loads persisted entities for a bounded set of normalized names.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get_by_names(
        &self,
        item_type: &str,
        names: &[String],
    ) -> Result<Vec<base_item::Model>, ItemByNameStoreError> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let item_types = supported_item_types(item_type)?;
        let clean_names = names
            .iter()
            .map(|name| name.clean_value())
            .collect::<Vec<_>>();
        Ok(base_item::Entity::find()
            .filter(base_item::Column::ItemType.is_in(item_types))
            .filter(base_item::Column::CleanName.is_in(clean_names))
            .order_by_asc(base_item::Column::Id)
            .all(self.database.as_ref())
            .await?)
    }

    /// Inserts the exact deterministic entity id, or returns the row inserted
    /// by a concurrent resolver. Unlike scan reconciliation, an unrelated
    /// legacy row with the same clean name does not replace this official id.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported or conflicting persisted types and
    /// database failures.
    pub async fn ensure(
        &self,
        entity: &NewItemByNameEntity,
    ) -> Result<base_item::Model, ItemByNameStoreError> {
        let item_types = supported_item_types(&entity.item_type)?;
        let transaction = self.database.begin().await?;
        transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO jellyfin.base_items (\
                     id, item_type, name, sort_name, path, is_folder, \
                     presentation_unique_key, date_created, date_modified\
                 ) VALUES ($1, $2, $3, $3, $4, \
                           CASE WHEN $2 IN ('MusicArtist', 'Person') THEN false ELSE true END, \
                           $5, $6, $7) \
                 ON CONFLICT (id) DO NOTHING",
                vec![
                    entity.id.into(),
                    entity.item_type.clone().into(),
                    entity.name.clone().into(),
                    entity.path.clone().into(),
                    entity.presentation_unique_key.clone().into(),
                    entity.date_created.into(),
                    entity.date_modified.into(),
                ],
            ))
            .await?;
        let item = base_item::Entity::find_by_id(entity.id)
            .one(&transaction)
            .await?
            .ok_or(ItemByNameStoreError::MissingInsertedItem)?;
        if !item_types
            .iter()
            .any(|item_type| item.item_type.eq_ignore_ascii_case(item_type))
        {
            return Err(ItemByNameStoreError::IncompatibleExistingItem);
        }
        transaction.commit().await?;
        Ok(item)
    }
}

fn supported_item_types(item_type: &str) -> Result<Vec<String>, ItemByNameStoreError> {
    if !matches!(
        item_type,
        "Genre" | "MusicGenre" | "MusicArtist" | "Person" | "Studio" | "Year"
    ) {
        return Err(ItemByNameStoreError::UnsupportedType(item_type.to_owned()));
    }
    Ok(expand_item_type_aliases(&[item_type.to_owned()]))
}
