use std::collections::{HashMap, HashSet};

use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, SqlErr, Statement, TransactionTrait,
    Value as SeaValue, sea_query::OnConflict,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    pub start_index: u64,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonPage {
    pub people: Vec<person::Model>,
    pub total_record_count: u64,
    pub start_index: u64,
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
        if let Some(limit) = query.limit {
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
            .all(&self.database)
            .await?,
            total_record_count: u64::try_from(count).unwrap_or_default(),
            start_index: query.start_index,
        })
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
        if query.recursive {
            push_bind(
                sql,
                values,
                parent_id,
                " AND item.id IN (SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
                  WHERE closure.parent_item_id = ",
            );
            sql.push(')');
        } else {
            push_bind(sql, values, parent_id, " AND item.parent_id = ");
        }
    }
    if let Some(item_id) = query.appears_in_item_id {
        push_bind(sql, values, item_id, " AND item.id = ");
    }
    append_string_list_filter(
        sql,
        values,
        "item.item_type",
        &query.include_item_types,
        false,
    );
    append_string_list_filter(
        sql,
        values,
        "item.item_type",
        &query.exclude_item_types,
        true,
    );
    append_string_list_filter(sql, values, "item.media_type", &query.media_types, false);
    append_media_class_filter(sql, query.is_movie, "IsMovie", &["Movie", "Trailer"]);
    append_media_class_filter(sql, query.is_series, "IsSeries", &["Series"]);
    append_tag_class_filter(sql, query.is_sports, "sports");
    append_tag_class_filter(sql, query.is_news, "news");
    append_tag_class_filter(sql, query.is_kids, "kids");
    append_string_list_filter(sql, values, "map.person_type", &query.person_types, false);
    append_string_list_filter(
        sql,
        values,
        "map.person_type",
        &query.exclude_person_types,
        true,
    );
    if let Some(is_favorite) = query.is_favorite {
        let Some(user_id) = query.user_id else {
            return;
        };
        if is_favorite {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE data.is_favorite = true AND data.user_id = ",
            );
        } else {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id NOT IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE data.is_favorite = true AND data.user_id = ",
            );
        }
        sql.push(')');
    }
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
        values.push(item.clone().into());
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
