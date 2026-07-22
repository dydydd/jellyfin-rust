use std::collections::{HashMap, HashSet};

use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, SqlErr, Statement, TransactionTrait,
    sea_query::OnConflict,
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, person, person_base_item_map};

#[derive(Debug, Clone, PartialEq)]
pub struct NewPerson {
    pub name: String,
    pub provider_ids: Value,
}

impl NewPerson {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            provider_ids: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonCredit {
    pub person: person::Model,
    pub person_type: String,
    pub role: String,
    pub sort_order: Option<i32>,
    pub list_order: i32,
}

#[derive(Debug, Error)]
pub enum PersonError {
    #[error("person name cannot be empty")]
    InvalidName,
    #[error("provider IDs must be a JSON object")]
    InvalidProviderIds,
    #[error("person type cannot be empty")]
    InvalidPersonType,
    #[error("person order cannot be negative")]
    InvalidOrder,
    #[error("base item was not found")]
    ItemNotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed canonical people and their ordered base-item credits.
#[derive(Clone)]
pub struct PersonRepository {
    database: DatabaseConnection,
}

impl PersonRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Atomically inserts a person or merges provider IDs into the existing
    /// Unicode-normalized identity.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn upsert(&self, input: NewPerson) -> Result<person::Model, PersonError> {
        upsert_on(&self.database, input).await
    }

    /// Finds a person using exact case-sensitive display text.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn get_exact(&self, name: &str) -> Result<Option<person::Model>, PersonError> {
        let name = validate_name(name)?;
        Ok(person::Entity::find()
            .filter(person::Column::Name.eq(name))
            .one(&self.database)
            .await?)
    }

    /// Finds a person using Jellyfin's Unicode-aware clean-name rules.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn get_normalized(&self, name: &str) -> Result<Option<person::Model>, PersonError> {
        let clean_name = clean_name(name)?;
        Ok(person::Entity::find()
            .filter(person::Column::CleanName.eq(clean_name))
            .one(&self.database)
            .await?)
    }

    /// Finds people containing an exact provider identifier pair.
    ///
    /// The JSONB containment predicate is served by `people_provider_ids_gin_idx`.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn by_provider_id(
        &self,
        provider: &str,
        provider_id: &str,
    ) -> Result<Vec<person::Model>, PersonError> {
        if provider.trim().is_empty() || provider_id.trim().is_empty() {
            return Err(PersonError::InvalidProviderIds);
        }
        let contained = serde_json::json!({ provider: provider_id });
        Ok(person::Entity::find()
            .from_raw_sql(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT * FROM jellyfin.people WHERE provider_ids @> $1::jsonb ORDER BY clean_name, id",
                [contained.into()],
            ))
            .all(&self.database)
            .await?)
    }

    /// Links a canonical person to a base item with role and ordering metadata.
    /// Repeated writes update ordering without creating duplicate credits.
    ///
    /// # Errors
    ///
    /// Returns `ItemNotFound`, validation, or database errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn link(
        &self,
        item_id: Uuid,
        person: NewPerson,
        person_type: &str,
        role: Option<&str>,
        sort_order: Option<i32>,
        list_order: i32,
    ) -> Result<person::Model, PersonError> {
        validate_credit(person_type, sort_order, list_order)?;
        let transaction = self.database.begin().await?;
        if base_item::Entity::find_by_id(item_id)
            .one(&transaction)
            .await?
            .is_none()
        {
            return Err(PersonError::ItemNotFound);
        }
        let person = upsert_on(&transaction, person).await?;
        person_base_item_map::Entity::insert(person_base_item_map::ActiveModel {
            item_id: sea_orm::Set(item_id),
            person_id: sea_orm::Set(person.id),
            person_type: sea_orm::Set(person_type.trim().to_owned()),
            role: sea_orm::Set(role.unwrap_or_default().trim().to_owned()),
            sort_order: sea_orm::Set(sort_order),
            list_order: sea_orm::Set(list_order),
        })
        .on_conflict(
            OnConflict::columns([
                person_base_item_map::Column::ItemId,
                person_base_item_map::Column::PersonId,
                person_base_item_map::Column::PersonType,
                person_base_item_map::Column::Role,
            ])
            .update_columns([
                person_base_item_map::Column::SortOrder,
                person_base_item_map::Column::ListOrder,
            ])
            .to_owned(),
        )
        .exec_without_returning(&transaction)
        .await
        .map_err(map_database_error)?;
        transaction.commit().await?;
        Ok(person)
    }

    /// Loads an item's credits in official list order.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn people_for_item(&self, item_id: Uuid) -> Result<Vec<PersonCredit>, PersonError> {
        let mappings = person_base_item_map::Entity::find()
            .filter(person_base_item_map::Column::ItemId.eq(item_id))
            .order_by_asc(person_base_item_map::Column::ListOrder)
            .order_by_asc(person_base_item_map::Column::PersonId)
            .all(&self.database)
            .await?;
        if mappings.is_empty() {
            return Ok(Vec::new());
        }
        let people = person::Entity::find()
            .filter(person::Column::Id.is_in(mappings.iter().map(|mapping| mapping.person_id)))
            .all(&self.database)
            .await?;
        let by_id: HashMap<Uuid, person::Model> = people
            .into_iter()
            .map(|person| (person.id, person))
            .collect();
        Ok(mappings
            .into_iter()
            .filter_map(|mapping| {
                by_id
                    .get(&mapping.person_id)
                    .cloned()
                    .map(|person| PersonCredit {
                        person,
                        person_type: mapping.person_type,
                        role: mapping.role,
                        sort_order: mapping.sort_order,
                        list_order: mapping.list_order,
                    })
            })
            .collect())
    }

    /// Loads distinct base items credited to a normalized person in stable
    /// item sort order.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn items_for_person(&self, name: &str) -> Result<Vec<base_item::Model>, PersonError> {
        let Some(person) = self.get_normalized(name).await? else {
            return Ok(Vec::new());
        };
        let item_ids: HashSet<Uuid> = person_base_item_map::Entity::find()
            .filter(person_base_item_map::Column::PersonId.eq(person.id))
            .all(&self.database)
            .await?
            .into_iter()
            .map(|mapping| mapping.item_id)
            .collect();
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Id.is_in(item_ids))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .all(&self.database)
            .await?)
    }

    /// Deletes one person and cascades every credit.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn delete(&self, id: Uuid) -> Result<bool, PersonError> {
        Ok(person::Entity::delete_by_id(id)
            .exec(&self.database)
            .await?
            .rows_affected
            == 1)
    }
}

async fn upsert_on<C>(connection: &C, input: NewPerson) -> Result<person::Model, PersonError>
where
    C: ConnectionTrait,
{
    let name = validate_name(&input.name)?;
    let clean_name = clean_name(name)?;
    if !input.provider_ids.is_object() {
        return Err(PersonError::InvalidProviderIds);
    }
    let statement = Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO jellyfin.people (id, name, clean_name, provider_ids)
           VALUES ($1, $2, $3, $4::jsonb)
           ON CONFLICT (clean_name) DO UPDATE
               SET provider_ids = jellyfin.people.provider_ids || EXCLUDED.provider_ids
           RETURNING id, name, clean_name, provider_ids,
                     date_created, date_modified, row_version",
        [
            Uuid::new_v4().into(),
            name.into(),
            clean_name.into(),
            input.provider_ids.into(),
        ],
    );
    person::Model::find_by_statement(statement)
        .one(connection)
        .await
        .map_err(map_database_error)?
        .ok_or_else(|| {
            PersonError::Database(DbErr::RecordNotFound(
                "person upsert returned no row".to_owned(),
            ))
        })
}

fn validate_name(name: &str) -> Result<&str, PersonError> {
    let name = name.trim();
    if name.is_empty() {
        Err(PersonError::InvalidName)
    } else {
        Ok(name)
    }
}

fn clean_name(name: &str) -> Result<String, PersonError> {
    let clean = validate_name(name)?.clean_value();
    if clean.is_empty() {
        Err(PersonError::InvalidName)
    } else {
        Ok(clean)
    }
}

fn validate_credit(
    person_type: &str,
    sort_order: Option<i32>,
    list_order: i32,
) -> Result<(), PersonError> {
    if person_type.trim().is_empty() {
        return Err(PersonError::InvalidPersonType);
    }
    if list_order < 0 || sort_order.is_some_and(|order| order < 0) {
        return Err(PersonError::InvalidOrder);
    }
    Ok(())
}

fn map_database_error(error: DbErr) -> PersonError {
    if matches!(
        error.sql_err(),
        Some(SqlErr::ForeignKeyConstraintViolation(_))
    ) && error.to_string().contains("people_map_item_fkey")
    {
        PersonError::ItemNotFound
    } else {
        PersonError::Database(error)
    }
}
