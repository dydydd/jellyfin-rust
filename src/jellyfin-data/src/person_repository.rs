use std::collections::{HashMap, HashSet};

use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, SqlErr, Statement, TransactionTrait, Value as SeaValue, sea_query::OnConflict,
};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    BaseItemQuery, NewItemByNameEntity,
    entities::{base_item, person, person_base_item_map},
    item_types::expand_item_type_aliases,
};

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
pub struct NewPersonCredit {
    pub person: NewPerson,
    pub person_type: String,
    pub role: String,
    pub sort_order: Option<i32>,
    pub list_order: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonCredit {
    pub person: person::Model,
    pub person_type: String,
    pub role: String,
    pub sort_order: Option<i32>,
    pub list_order: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PersonQuery {
    pub ids: Vec<Uuid>,
    pub parent_id: Option<Uuid>,
    pub recursive: bool,
    pub appears_in_item_id: Option<Uuid>,
    pub search_term: Option<String>,
    pub include_item_types: Vec<String>,
    pub exclude_item_types: Vec<String>,
    pub media_types: Vec<String>,
    pub is_movie: Option<bool>,
    pub is_series: Option<bool>,
    pub is_news: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_sports: Option<bool>,
    pub person_types: Vec<String>,
    pub exclude_person_types: Vec<String>,
    pub is_favorite: Option<bool>,
    pub user_id: Option<Uuid>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_starts_with: Option<String>,
    pub name_less_than: Option<String>,
    pub access_filter: Option<BaseItemQuery>,
    pub start_index: u64,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonPage {
    pub people: Vec<person::Model>,
    pub total_record_count: u64,
    pub start_index: u64,
}

/// One canonical Person item prepared from a referenced people row.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalPersonEntity {
    pub person_id: Uuid,
    pub provider_ids: Value,
    pub entity: NewItemByNameEntity,
}

/// Set-based changes committed for one Person reconciliation batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PersonReconciliationBatchResult {
    pub created_items: u64,
    pub updated_items: u64,
    pub copied_images: u64,
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
    database: crate::SharedDatabase,
}

impl PersonRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
    }

    /// Atomically inserts a person or merges provider IDs into the existing
    /// Unicode-normalized identity.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors.
    pub async fn upsert(&self, input: NewPerson) -> Result<person::Model, PersonError> {
        upsert_on(self.database.as_ref(), input).await
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
            .one(self.database.as_ref())
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
            .one(self.database.as_ref())
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
            .all(self.database.as_ref())
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

    /// Atomically replaces every credit for one item and returns the canonical
    /// people in input order.
    ///
    /// Validation happens before the transaction starts. The item existence
    /// check, old-map deletion, person upserts, and replacement maps then share
    /// one transaction instead of opening one transaction per credit.
    ///
    /// # Errors
    ///
    /// Returns `ItemNotFound`, validation, or database errors. No existing
    /// credit is removed when validation or a database operation fails.
    pub async fn replace_credits(
        &self,
        item_id: Uuid,
        credits: Vec<NewPersonCredit>,
    ) -> Result<Vec<person::Model>, PersonError> {
        for credit in &credits {
            clean_name(&credit.person.name)?;
            if !credit.person.provider_ids.is_object() {
                return Err(PersonError::InvalidProviderIds);
            }
            validate_credit(&credit.person_type, credit.sort_order, credit.list_order)?;
        }

        let transaction = self.database.begin().await?;
        if base_item::Entity::find_by_id(item_id)
            .one(&transaction)
            .await?
            .is_none()
        {
            return Err(PersonError::ItemNotFound);
        }
        person_base_item_map::Entity::delete_many()
            .filter(person_base_item_map::Column::ItemId.eq(item_id))
            .exec(&transaction)
            .await?;

        let mut people = Vec::with_capacity(credits.len());
        for credit in credits {
            let person = upsert_on(&transaction, credit.person).await?;
            person_base_item_map::Entity::insert(person_base_item_map::ActiveModel {
                item_id: sea_orm::Set(item_id),
                person_id: sea_orm::Set(person.id),
                person_type: sea_orm::Set(credit.person_type.trim().to_owned()),
                role: sea_orm::Set(credit.role.trim().to_owned()),
                sort_order: sea_orm::Set(credit.sort_order),
                list_order: sea_orm::Set(credit.list_order),
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
            people.push(person);
        }
        transaction.commit().await?;
        Ok(people)
    }

    /// Removes every persisted credit for one item.
    ///
    /// # Errors
    ///
    /// Returns a database error when the delete fails.
    pub async fn clear_credits(&self, item_id: Uuid) -> Result<usize, PersonError> {
        Ok(person_base_item_map::Entity::delete_many()
            .filter(person_base_item_map::Column::ItemId.eq(item_id))
            .exec(self.database.as_ref())
            .await?
            .rows_affected
            .try_into()
            .unwrap_or(usize::MAX))
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
            .all(self.database.as_ref())
            .await?;
        if mappings.is_empty() {
            return Ok(Vec::new());
        }
        let people = person::Entity::find()
            .filter(person::Column::Id.is_in(mappings.iter().map(|mapping| mapping.person_id)))
            .all(self.database.as_ref())
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

    /// Loads credits for many items in one query, grouped by item id.
    ///
    /// Credits keep each item's official list order.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn people_for_items(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<PersonCredit>>, PersonError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut mappings = person_base_item_map::Entity::find()
            .filter(person_base_item_map::Column::ItemId.is_in(item_ids.iter().copied()))
            .order_by_asc(person_base_item_map::Column::ListOrder)
            .order_by_asc(person_base_item_map::Column::PersonId)
            .all(self.database.as_ref())
            .await?;
        let person_ids = mappings
            .iter()
            .map(|mapping| mapping.person_id)
            .collect::<HashSet<_>>();
        let people = if person_ids.is_empty() {
            Vec::new()
        } else {
            person::Entity::find()
                .filter(person::Column::Id.is_in(person_ids))
                .all(self.database.as_ref())
                .await?
        };
        let by_id: HashMap<Uuid, person::Model> = people
            .into_iter()
            .map(|person| (person.id, person))
            .collect();
        let mut result = HashMap::<Uuid, Vec<PersonCredit>>::new();
        for mapping in mappings.drain(..) {
            let Some(person) = by_id.get(&mapping.person_id).cloned() else {
                continue;
            };
            result
                .entry(mapping.item_id)
                .or_default()
                .push(PersonCredit {
                    person,
                    person_type: mapping.person_type,
                    role: mapping.role,
                    sort_order: mapping.sort_order,
                    list_order: mapping.list_order,
                });
        }
        Ok(result)
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
            .all(self.database.as_ref())
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
            .all(self.database.as_ref())
            .await?)
    }

    /// Lists distinct people credited to filtered base items.
    ///
    /// # Errors
    ///
    /// Returns a database error when the distinct-people query fails.
    pub async fn query(&self, query: &PersonQuery) -> Result<PersonPage, PersonError> {
        let (cte, values) = people_cte(query);
        let count = self
            .database
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("{cte} SELECT COUNT(*) AS total_record_count FROM matched"),
                values.clone(),
            ))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("person count returned no row".to_owned()))?
            .try_get::<i64>("", "total_record_count")?;

        let mut page_values = values;
        let mut page_sql = format!(
            "{cte} SELECT id, name, clean_name, provider_ids, date_created, date_modified, row_version \
             FROM matched ORDER BY clean_name, id"
        );
        push_bind(
            &mut page_sql,
            &mut page_values,
            i64::try_from(query.start_index).unwrap_or(i64::MAX),
            " OFFSET ",
        );
        if let Some(limit) = effective_limit(query.limit) {
            push_bind(
                &mut page_sql,
                &mut page_values,
                i64::try_from(limit).unwrap_or(i64::MAX),
                " LIMIT ",
            );
        }
        Ok(PersonPage {
            people: person::Model::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                page_sql,
                page_values,
            ))
            .all(self.database.as_ref())
            .await?,
            total_record_count: u64::try_from(count).unwrap_or_default(),
            start_index: query.start_index,
        })
    }

    /// Keyset-pages distinct people that are still referenced by at least one
    /// base-item credit.
    ///
    /// The map foreign keys already prevent dangling credit rows. Unreferenced
    /// people are conservatively ignored rather than deleted by reconciliation.
    ///
    /// # Errors
    ///
    /// Returns a database error when the page cannot be loaded.
    pub async fn referenced_page_after(
        &self,
        after: Option<(&str, Uuid)>,
        limit: usize,
    ) -> Result<Vec<person::Model>, PersonError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(
            person::Model::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
            SELECT person.id, person.name, person.clean_name, person.provider_ids,
                   person.date_created, person.date_modified, person.row_version
            FROM jellyfin.people AS person
            WHERE ($1::text IS NULL OR (person.clean_name, person.id) > ($1, $2))
              AND EXISTS (
                  SELECT 1
                  FROM jellyfin.people_base_item_map AS credit
                  WHERE credit.person_id = person.id
              )
            ORDER BY person.clean_name, person.id
            LIMIT $3
            ",
                [
                    after.map(|(clean_name, _)| clean_name.to_owned()).into(),
                    after.map_or(Uuid::nil(), |(_, id)| id).into(),
                    i64::try_from(limit).unwrap_or(i64::MAX).into(),
                ],
            ))
            .all(self.database.as_ref())
            .await?,
        )
    }

    /// Ensures and conservatively hydrates one bounded batch of canonical
    /// Person items in a single transaction.
    ///
    /// Canonical values always win. For empty fields only, the best legacy row
    /// is chosen by locked state, metadata, image presence, modification time,
    /// and id. Provider IDs merge without replacing existing keys, while image
    /// rows copy by reference with no source-row or file mutation.
    ///
    /// # Errors
    ///
    /// Returns invalid-provider or database errors. No partial batch is
    /// committed when any statement fails.
    #[allow(clippy::too_many_lines)]
    pub async fn reconcile_canonical_batch(
        &self,
        entries: &[CanonicalPersonEntity],
    ) -> Result<PersonReconciliationBatchResult, PersonError> {
        if entries.is_empty() {
            return Ok(PersonReconciliationBatchResult::default());
        }
        if entries.iter().any(|entry| !entry.provider_ids.is_object()) {
            return Err(PersonError::InvalidProviderIds);
        }
        let payload = Value::Array(
            entries
                .iter()
                .map(|entry| {
                    json!({
                        "person_id": entry.person_id,
                        "canonical_id": entry.entity.id,
                        "name": entry.entity.name,
                        "path": entry.entity.path,
                        "presentation_unique_key": entry.entity.presentation_unique_key,
                        "date_created": entry.entity.date_created,
                        "date_modified": entry.entity.date_modified,
                        "provider_ids": entry.provider_ids,
                    })
                })
                .collect(),
        );
        let transaction = self.database.begin().await?;
        let created_items = transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH input AS (
                    SELECT *
                    FROM jsonb_to_recordset($1::jsonb) AS entry(
                        person_id uuid,
                        canonical_id uuid,
                        name text,
                        path text,
                        presentation_unique_key text,
                        date_created timestamptz,
                        date_modified timestamptz,
                        provider_ids jsonb
                    )
                ), referenced_input AS (
                    SELECT input.*
                    FROM input
                    WHERE EXISTS (
                        SELECT 1
                        FROM jellyfin.people_base_item_map AS credit
                        WHERE credit.person_id = input.person_id
                    )
                )
                INSERT INTO jellyfin.base_items (
                    id, item_type, data, path, name, sort_name, is_folder,
                    is_virtual_item, presentation_unique_key, date_created, date_modified
                )
                SELECT canonical_id, 'Person',
                       CASE WHEN provider_ids = '{}'::jsonb THEN NULL
                            ELSE jsonb_build_object('ProviderIds', provider_ids) END,
                       path, name, name, false, false, presentation_unique_key,
                       date_created, date_modified
                FROM referenced_input
                ON CONFLICT (id) DO NOTHING
                ",
                [payload.clone().into()],
            ))
            .await?
            .rows_affected();

        let updated_items = transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH input AS (
                    SELECT *
                    FROM jsonb_to_recordset($1::jsonb) AS entry(
                        person_id uuid,
                        canonical_id uuid,
                        name text,
                        path text,
                        presentation_unique_key text,
                        date_created timestamptz,
                        date_modified timestamptz,
                        provider_ids jsonb
                    )
                ), referenced_input AS (
                    SELECT input.*
                    FROM input
                    WHERE EXISTS (
                        SELECT 1
                        FROM jellyfin.people_base_item_map AS credit
                        WHERE credit.person_id = input.person_id
                    )
                ),
                candidate AS (
                    SELECT input.*,
                           legacy.data AS legacy_data,
                           legacy.name AS legacy_name,
                           legacy.sort_name AS legacy_sort_name,
                           legacy.overview AS legacy_overview,
                           legacy.official_rating AS legacy_official_rating,
                           legacy.production_year AS legacy_production_year,
                           legacy.premiere_date AS legacy_premiere_date
                    FROM referenced_input AS input
                    LEFT JOIN LATERAL (
                        SELECT legacy.*
                        FROM jellyfin.base_items AS legacy
                        WHERE legacy.id <> input.canonical_id
                          AND legacy.item_type IN (
                              'Person', 'MediaBrowser.Controller.Entities.Person'
                          )
                          AND legacy.clean_name = jellyfin.normalize_search_text(input.name)
                        ORDER BY
                            CASE WHEN lower(COALESCE(legacy.data->>'IsLocked', 'false'))
                                      IN ('true', '1') THEN 1 ELSE 0 END DESC,
                            CASE WHEN legacy.data IS NOT NULL
                                      AND legacy.data <> '{}'::jsonb THEN 1 ELSE 0 END DESC,
                            CASE WHEN EXISTS (
                                SELECT 1 FROM jellyfin.base_item_images AS image
                                WHERE image.item_id = legacy.id
                            ) THEN 1 ELSE 0 END DESC,
                            legacy.date_modified DESC,
                            legacy.id
                        LIMIT 1
                    ) AS legacy ON true
                ),
                desired AS (
                    SELECT target.id,
                           'Person'::text AS item_type,
                           CASE
                               WHEN target.data IS NOT NULL
                                    AND jsonb_typeof(target.data) <> 'object'
                               THEN target.data
                               ELSE jsonb_set(
                                   (CASE WHEN jsonb_typeof(candidate.legacy_data) = 'object'
                                         THEN candidate.legacy_data ELSE '{}'::jsonb END)
                                   || COALESCE(target.data, '{}'::jsonb),
                                   '{ProviderIds}',
                                   merged_provider_ids.value,
                                   true
                               )
                           END AS data,
                           CASE WHEN NULLIF(btrim(target.path), '') IS NULL
                                THEN candidate.path ELSE target.path END AS path,
                           COALESCE(NULLIF(btrim(target.name), ''),
                                    NULLIF(btrim(candidate.legacy_name), ''),
                                    candidate.name) AS name,
                           COALESCE(NULLIF(btrim(target.sort_name), ''),
                                    NULLIF(btrim(candidate.legacy_sort_name), ''),
                                    candidate.name) AS sort_name,
                           COALESCE(NULLIF(btrim(target.overview), ''),
                                    NULLIF(btrim(candidate.legacy_overview), '')) AS overview,
                           COALESCE(NULLIF(btrim(target.official_rating), ''),
                                    NULLIF(btrim(candidate.legacy_official_rating), ''))
                               AS official_rating,
                           COALESCE(target.production_year, candidate.legacy_production_year)
                               AS production_year,
                           COALESCE(target.premiere_date, candidate.legacy_premiere_date)
                               AS premiere_date,
                           false AS is_folder,
                           false AS is_virtual_item,
                           COALESCE(NULLIF(btrim(target.presentation_unique_key), ''),
                                    candidate.presentation_unique_key)
                               AS presentation_unique_key
                    FROM candidate
                    JOIN jellyfin.base_items AS target
                      ON target.id = candidate.canonical_id
                    CROSS JOIN LATERAL (
                        SELECT COALESCE(
                            jsonb_object_agg(selected.key, selected.value),
                            '{}'::jsonb
                        ) AS value
                        FROM (
                            SELECT DISTINCT ON (lower(provider.key))
                                   provider.key, provider.value
                            FROM (
                                SELECT entry.key, entry.value, 1 AS priority
                                FROM jsonb_each(
                                    CASE
                                        WHEN jsonb_typeof(
                                            candidate.legacy_data->'ProviderIds'
                                        ) = 'object'
                                        THEN candidate.legacy_data->'ProviderIds'
                                        ELSE '{}'::jsonb
                                    END
                                ) AS entry
                                UNION ALL
                                SELECT entry.key, entry.value, 2 AS priority
                                FROM jsonb_each(candidate.provider_ids) AS entry
                                UNION ALL
                                SELECT entry.key, entry.value, 3 AS priority
                                FROM jsonb_each(
                                    CASE
                                        WHEN jsonb_typeof(target.data->'ProviderIds') = 'object'
                                        THEN target.data->'ProviderIds'
                                        ELSE '{}'::jsonb
                                    END
                                ) AS entry
                            ) AS provider
                            ORDER BY lower(provider.key), provider.priority DESC
                        ) AS selected
                    ) AS merged_provider_ids
                )
                UPDATE jellyfin.base_items AS target
                SET item_type = desired.item_type,
                    data = desired.data,
                    path = desired.path,
                    name = desired.name,
                    sort_name = desired.sort_name,
                    overview = desired.overview,
                    official_rating = desired.official_rating,
                    production_year = desired.production_year,
                    premiere_date = desired.premiere_date,
                    is_folder = desired.is_folder,
                    is_virtual_item = desired.is_virtual_item,
                    presentation_unique_key = desired.presentation_unique_key
                FROM desired
                WHERE target.id = desired.id
                  AND ROW(
                      target.item_type, target.data, target.path, target.name, target.sort_name,
                      target.overview, target.official_rating, target.production_year,
                      target.premiere_date, target.is_folder, target.is_virtual_item,
                      target.presentation_unique_key
                  ) IS DISTINCT FROM ROW(
                      desired.item_type, desired.data, desired.path, desired.name,
                      desired.sort_name, desired.overview, desired.official_rating,
                      desired.production_year, desired.premiere_date, desired.is_folder,
                      desired.is_virtual_item, desired.presentation_unique_key
                  )
                ",
                [payload.clone().into()],
            ))
            .await?
            .rows_affected();

        let copied_images = transaction
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                WITH input AS (
                    SELECT *
                    FROM jsonb_to_recordset($1::jsonb) AS entry(
                        person_id uuid,
                        canonical_id uuid,
                        name text,
                        path text,
                        presentation_unique_key text,
                        date_created timestamptz,
                        date_modified timestamptz,
                        provider_ids jsonb
                    )
                ), referenced_input AS (
                    SELECT input.*
                    FROM input
                    WHERE EXISTS (
                        SELECT 1
                        FROM jellyfin.people_base_item_map AS credit
                        WHERE credit.person_id = input.person_id
                    )
                ),
                candidate AS (
                    SELECT input.canonical_id, legacy.id AS legacy_id
                    FROM referenced_input AS input
                    JOIN LATERAL (
                        SELECT legacy.id
                        FROM jellyfin.base_items AS legacy
                        WHERE legacy.id <> input.canonical_id
                          AND legacy.item_type IN (
                              'Person', 'MediaBrowser.Controller.Entities.Person'
                          )
                          AND legacy.clean_name = jellyfin.normalize_search_text(input.name)
                        ORDER BY
                            CASE WHEN lower(COALESCE(legacy.data->>'IsLocked', 'false'))
                                      IN ('true', '1') THEN 1 ELSE 0 END DESC,
                            CASE WHEN legacy.data IS NOT NULL
                                      AND legacy.data <> '{}'::jsonb THEN 1 ELSE 0 END DESC,
                            CASE WHEN EXISTS (
                                SELECT 1 FROM jellyfin.base_item_images AS image
                                WHERE image.item_id = legacy.id
                            ) THEN 1 ELSE 0 END DESC,
                            legacy.date_modified DESC,
                            legacy.id
                        LIMIT 1
                    ) AS legacy ON true
                )
                INSERT INTO jellyfin.base_item_images (
                    item_id, image_type, image_index, path, date_modified,
                    width, height, blurhash
                )
                SELECT candidate.canonical_id, image.image_type, image.image_index,
                       image.path, image.date_modified, image.width, image.height, image.blurhash
                FROM candidate
                JOIN jellyfin.base_item_images AS image
                  ON image.item_id = candidate.legacy_id
                ON CONFLICT (item_id, image_type, image_index) DO NOTHING
                ",
                [payload.into()],
            ))
            .await?
            .rows_affected();

        transaction.commit().await?;
        Ok(PersonReconciliationBatchResult {
            created_items,
            updated_items,
            copied_images,
        })
    }

    /// Deletes one person and cascades every credit.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn delete(&self, id: Uuid) -> Result<bool, PersonError> {
        Ok(person::Entity::delete_by_id(id)
            .exec(self.database.as_ref())
            .await?
            .rows_affected
            == 1)
    }
}

const fn effective_limit(limit: Option<u64>) -> Option<u64> {
    match limit {
        Some(0) | None => None,
        Some(limit) => Some(limit),
    }
}

fn people_cte(query: &PersonQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH linked AS (\
             SELECT person.id, person.name, person.clean_name, person.provider_ids, \
                    person.date_created, person.date_modified, person.row_version \
             FROM jellyfin.people AS person \
             JOIN jellyfin.people_base_item_map AS map ON map.person_id = person.id \
             JOIN jellyfin.base_items AS item ON item.id = map.item_id \
             WHERE item.item_type <> 'PLACEHOLDER'",
    );
    append_people_item_filters(&mut sql, &mut values, query);
    append_person_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), matched AS (\
             SELECT id, name, clean_name, provider_ids, date_created, date_modified, row_version \
             FROM linked \
             GROUP BY id, name, clean_name, provider_ids, date_created, date_modified, row_version\
         )",
    );
    (sql, values)
}

fn append_people_item_filters(sql: &mut String, values: &mut Vec<SeaValue>, query: &PersonQuery) {
    if let Some(access_filter) = query.access_filter.as_ref()
        && let Some(condition) =
            crate::base_item_repository::policy_filter_sql("item", access_filter)
    {
        sql.push_str(" AND (");
        sql.push_str(&condition);
        sql.push(')');
    }
    if !query.ids.is_empty() {
        sql.push_str(" AND item.id IN (");
        for (index, item_id) in query.ids.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            values.push((*item_id).into());
            sql.push('$');
            sql.push_str(&values.len().to_string());
        }
        sql.push(')');
    }
    if let Some(parent_id) = query.parent_id {
        push_bind(
            sql,
            values,
            parent_id,
            " AND item.id IN (SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
              WHERE closure.parent_item_id = ",
        );
        sql.push(')');
    }
    if let Some(item_id) = query.appears_in_item_id {
        push_bind(sql, values, item_id, " AND item.id = ");
    }
    let include_item_types = expand_item_type_aliases(&query.include_item_types);
    append_string_list_filter(sql, values, "item.item_type", &include_item_types, false);
    let exclude_item_types = expand_item_type_aliases(&query.exclude_item_types);
    append_string_list_filter(sql, values, "item.item_type", &exclude_item_types, true);
    append_string_list_filter(sql, values, "item.media_type", &query.media_types, false);
    append_media_class_filter(sql, query.is_movie, "IsMovie", &["Movie", "Trailer"]);
    append_media_class_filter(sql, query.is_series, "IsSeries", &["Series"]);
    append_tag_class_filter(sql, query.is_sports, "sports");
    append_tag_class_filter(sql, query.is_news, "news");
    append_tag_class_filter(sql, query.is_kids, "kids");
    append_person_type_filter(sql, values, &query.person_types, false);
    append_person_type_filter(sql, values, &query.exclude_person_types, true);
    if let Some(is_favorite) = query.is_favorite {
        let Some(user_id) = query.user_id else {
            return;
        };
        push_bind(
            sql,
            values,
            user_id,
            " AND EXISTS (
                SELECT 1 FROM jellyfin.user_data AS data
                JOIN jellyfin.base_items AS person_item ON person_item.id = data.item_id
                WHERE person_item.item_type = 'Person'
                  AND person_item.name = person.name
                  AND data.user_id = ",
        );
        push_bind(sql, values, is_favorite, " AND data.is_favorite = ");
        sql.push(')');
    }
}

fn append_person_type_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    items: &[String],
    negated: bool,
) {
    let valid_items = items
        .iter()
        .filter(|item| is_valid_person_type(item))
        .cloned()
        .collect::<Vec<_>>();
    append_string_list_filter(sql, values, "map.person_type", &valid_items, negated);
}

fn is_valid_person_type(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().all(char::is_alphanumeric)
}

fn append_tag_class_filter(sql: &mut String, expected: Option<bool>, clean_tag: &'static str) {
    let Some(expected) = expected else {
        return;
    };
    let expression = tag_class_expression(clean_tag);
    if expected {
        sql.push_str(" AND ");
        sql.push_str(&expression);
    } else {
        sql.push_str(" AND NOT ");
        sql.push_str(&expression);
    }
}

fn tag_class_expression(clean_tag: &'static str) -> String {
    format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS tag_map \
            JOIN jellyfin.item_values AS tag_value \
              ON tag_value.item_value_id = tag_map.item_value_id \
            WHERE tag_map.item_id = item.id \
              AND tag_value.type = 4 \
              AND tag_value.clean_value = '{clean_tag}'\
        )"
    )
}

fn append_media_class_filter(
    sql: &mut String,
    expected: Option<bool>,
    json_key: &'static str,
    item_types: &'static [&'static str],
) {
    let Some(expected) = expected else {
        return;
    };
    let expression = media_class_expression(json_key, item_types);
    if expected {
        sql.push_str(" AND ");
        sql.push_str(&expression);
    } else {
        sql.push_str(" AND NOT ");
        sql.push_str(&expression);
    }
}

fn media_class_expression(json_key: &'static str, item_types: &'static [&'static str]) -> String {
    let mut expression = String::from("(item.item_type IN (");
    for (index, item_type) in item_types.iter().enumerate() {
        if index > 0 {
            expression.push_str(", ");
        }
        expression.push('\'');
        expression.push_str(item_type);
        expression.push('\'');
    }
    expression.push_str(") OR COALESCE(lower(item.data ->> '");
    expression.push_str(json_key);
    expression.push_str("') = 'true', false))");
    expression
}

fn append_person_filters(sql: &mut String, values: &mut Vec<SeaValue>, query: &PersonQuery) {
    if let Some(search_term) = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        push_bind(
            sql,
            values,
            postgres_contains_pattern(&search_term.clean_value()),
            " AND person.clean_name ILIKE ",
        );
    }
    if let Some(name) = query
        .name_starts_with
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        push_bind(
            sql,
            values,
            format!("{}%", escape_like(&name.clean_value())),
            " AND person.clean_name ILIKE ",
        );
    }
    if let Some(name) = query
        .name_starts_with_or_greater
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        push_bind(
            sql,
            values,
            name.clean_value(),
            " AND person.clean_name >= ",
        );
    }
    if let Some(name) = query
        .name_less_than
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        push_bind(sql, values, name.clean_value(), " AND person.clean_name < ");
    }
}

fn append_string_list_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    column: &str,
    items: &[String],
    negated: bool,
) {
    if items.is_empty() {
        return;
    }
    let operator = if negated { "NOT IN" } else { "IN" };
    sql.push_str(" AND ");
    sql.push_str(column);
    sql.push(' ');
    sql.push_str(operator);
    sql.push_str(" (");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        values.push(item.as_str().into());
        sql.push('$');
        sql.push_str(&values.len().to_string());
    }
    sql.push(')');
}

fn push_bind<T: Into<SeaValue>>(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    value: T,
    prefix: &str,
) {
    values.push(value.into());
    sql.push_str(prefix);
    sql.push('$');
    sql.push_str(&values.len().to_string());
}

fn postgres_contains_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    escaped.push_str(&escape_like(value));
    escaped.push('%');
    escaped
}

fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
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

#[cfg(test)]
mod tests {
    use sea_orm::Value as SeaValue;

    use super::{PersonQuery, append_people_item_filters};

    #[test]
    fn person_item_filters_expand_type_aliases_and_preserve_plugin_types() {
        let query = PersonQuery {
            include_item_types: vec!["movie".to_owned(), "Plugin.Media.SpecialItem".to_owned()],
            exclude_item_types: vec!["episode".to_owned()],
            ..Default::default()
        };
        let mut sql = String::new();
        let mut values = Vec::new();

        append_people_item_filters(&mut sql, &mut values, &query);

        assert!(sql.contains("item.item_type IN ($1, $2, $3)"));
        assert!(sql.contains("item.item_type NOT IN ($4, $5)"));
        assert_eq!(
            string_values(&values),
            [
                "Movie",
                "MediaBrowser.Controller.Entities.Movies.Movie",
                "Plugin.Media.SpecialItem",
                "Episode",
                "MediaBrowser.Controller.Entities.TV.Episode",
            ]
        );
    }

    fn string_values(values: &[SeaValue]) -> Vec<&str> {
        values
            .iter()
            .map(|value| match value {
                SeaValue::String(Some(value)) => value.as_str(),
                value => panic!("expected string bind value, got {value:?}"),
            })
            .collect()
    }
}
