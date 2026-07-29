use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, FromQueryResult, QueryFilter, QueryOrder, Statement, TransactionTrait,
    Value as SeaValue, sea_query::OnConflict,
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{base_item, item_value, item_value_map};

#[derive(Debug, Error)]
pub enum ItemValueError {
    #[error("item value cannot be empty")]
    InvalidValue,
    #[error("base item was not found")]
    ItemNotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemValueQuery {
    pub ids: Vec<Uuid>,
    pub parent_id: Option<Uuid>,
    pub recursive: bool,
    pub search_term: Option<String>,
    pub include_item_types: Vec<String>,
    pub exclude_item_types: Vec<String>,
    pub media_types: Vec<String>,
    pub is_movie: Option<bool>,
    pub is_series: Option<bool>,
    pub is_news: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_sports: Option<bool>,
    pub is_favorite: Option<bool>,
    pub user_id: Option<Uuid>,
    pub by_name_item_type: Option<String>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_starts_with: Option<String>,
    pub name_less_than: Option<String>,
    pub start_index: u64,
    pub limit: Option<u64>,
    pub order: ItemValueOrder,
    pub descending: bool,
    pub enable_total_record_count: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ItemValueOrder {
    #[default]
    CleanValue,
    Random,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemValueInfo {
    pub id: Uuid,
    pub value: String,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemValuePage {
    pub values: Vec<ItemValueInfo>,
    pub total_record_count: u64,
    pub start_index: u64,
}

/// PostgreSQL-backed normalized item values and their many-to-many base-item
/// associations.
#[derive(Clone)]
pub struct ItemValueRepository {
    database: DatabaseConnection,
}

impl ItemValueRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Inserts a canonical item value, or returns the existing row whose
    /// normalized value is equivalent.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn upsert(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<item_value::Model, ItemValueError> {
        upsert_on(&self.database, value_type, value).await
    }

    /// Finds a value using exact, case-sensitive display text.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn get_exact(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<Option<item_value::Model>, ItemValueError> {
        let value = validate_value(value)?;
        Ok(item_value::Entity::find()
            .filter(item_value::Column::ValueType.eq(value_type))
            .filter(item_value::Column::Value.eq(value))
            .one(&self.database)
            .await?)
    }

    /// Finds a value using Jellyfin's Unicode-aware clean-value rules.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn get_normalized(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<Option<item_value::Model>, ItemValueError> {
        let value = validate_value(value)?;
        let clean_value = value.clean_value();
        Ok(item_value::Entity::find()
            .filter(item_value::Column::ValueType.eq(value_type))
            .filter(item_value::Column::CleanValue.eq(clean_value))
            .one(&self.database)
            .await?)
    }

    /// Loads one normalized value by stable identifier and type.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn get_by_id(
        &self,
        id: Uuid,
        value_type: item_value::ItemValueType,
    ) -> Result<Option<item_value::Model>, ItemValueError> {
        Ok(item_value::Entity::find_by_id(id)
            .filter(item_value::Column::ValueType.eq(value_type))
            .one(&self.database)
            .await?)
    }

    /// Atomically creates or reuses a normalized value and links it to a base
    /// item. Repeated and concurrent links are idempotent.
    ///
    /// # Errors
    ///
    /// Returns `ItemNotFound` when the base item is absent, or a validation or
    /// database error.
    pub async fn link(
        &self,
        item_id: Uuid,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<item_value::Model, ItemValueError> {
        let transaction = self.database.begin().await?;
        if base_item::Entity::find_by_id(item_id)
            .one(&transaction)
            .await?
            .is_none()
        {
            return Err(ItemValueError::ItemNotFound);
        }
        let item_value = upsert_on(&transaction, value_type, value).await?;
        item_value_map::Entity::insert(item_value_map::ActiveModel {
            item_value_id: Set(item_value.item_value_id),
            item_id: Set(item_id),
        })
        .on_conflict(
            OnConflict::columns([
                item_value_map::Column::ItemValueId,
                item_value_map::Column::ItemId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(&transaction)
        .await?;
        transaction.commit().await?;
        Ok(item_value)
    }

    /// Loads values of one type attached to an item in normalized name order.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn values_for_item(
        &self,
        item_id: Uuid,
        value_type: item_value::ItemValueType,
    ) -> Result<Vec<item_value::Model>, ItemValueError> {
        let ids = item_value_map::Entity::find()
            .filter(item_value_map::Column::ItemId.eq(item_id))
            .all(&self.database)
            .await?
            .into_iter()
            .map(|mapping| mapping.item_value_id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(item_value::Entity::find()
            .filter(item_value::Column::ItemValueId.is_in(ids))
            .filter(item_value::Column::ValueType.eq(value_type))
            .order_by_asc(item_value::Column::CleanValue)
            .order_by_asc(item_value::Column::ItemValueId)
            .all(&self.database)
            .await?)
    }

    /// Loads base items attached to a normalized value in stable sort order.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error.
    pub async fn items_for_value(
        &self,
        value_type: item_value::ItemValueType,
        value: &str,
    ) -> Result<Vec<base_item::Model>, ItemValueError> {
        let Some(value) = self.get_normalized(value_type, value).await? else {
            return Ok(Vec::new());
        };
        let ids = item_value_map::Entity::find()
            .filter(item_value_map::Column::ItemValueId.eq(value.item_value_id))
            .all(&self.database)
            .await?
            .into_iter()
            .map(|mapping| mapping.item_id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Id.is_in(ids))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .all(&self.database)
            .await?)
    }

    /// Selects random audio items sharing at least one normalized genre.
    ///
    /// PostgreSQL performs the set intersection, deduplication, randomization,
    /// and limit in one query so the candidate library is never loaded into
    /// application memory.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn random_audio_for_genres(
        &self,
        genre_ids: &[Uuid],
        limit: u64,
    ) -> Result<Vec<base_item::Model>, ItemValueError> {
        if genre_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut values = genre_ids
            .iter()
            .copied()
            .map(SeaValue::from)
            .collect::<Vec<_>>();
        let placeholders = (1..=genre_ids.len())
            .map(|index| format!("${index}::uuid"))
            .collect::<Vec<_>>()
            .join(", ");
        values.push(limit.into());
        Ok(
            base_item::Model::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "SELECT item.* \
                 FROM jellyfin.base_items AS item \
                 WHERE item.item_type = 'Audio' \
                   AND item.is_virtual_item = false \
                   AND EXISTS ( \
                       SELECT 1 FROM jellyfin.item_value_map AS map \
                       INNER JOIN jellyfin.item_values AS value \
                         ON value.item_value_id = map.item_value_id \
                       WHERE map.item_id = item.id \
                         AND value.type = 2 \
                         AND value.item_value_id IN ({placeholders}) \
                   ) \
                 ORDER BY random() \
                 LIMIT ${}::bigint",
                    genre_ids.len() + 1
                ),
                values,
            ))
            .all(&self.database)
            .await?,
        )
    }

    /// Deletes all inherited tag associations (post-scan cleanup).
    ///
    /// # Errors
    ///
    /// Returns a database error when the delete fails.
    pub async fn clear_inherited_tags(&self) -> Result<(), ItemValueError> {
        item_value::Entity::delete_many()
            .filter(item_value::Column::ValueType.eq(item_value::ItemValueType::InheritedTags))
            .exec(&self.database)
            .await?;
        Ok(())
    }

    /// Lists item-by-name values that are attached to filtered base items.
    ///
    /// # Errors
    ///
    /// Returns a database error when the distinct-value query fails.
    pub async fn query_values(
        &self,
        value_type: item_value::ItemValueType,
        query: &ItemValueQuery,
    ) -> Result<ItemValuePage, ItemValueError> {
        let (cte, values) = item_values_cte(value_type, query);
        let count = if total_count_enabled(query) {
            Some(
                self.database
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        format!("{cte} SELECT COUNT(*) AS total_record_count FROM values"),
                        values.clone(),
                    ))
                    .await?
                    .ok_or_else(|| {
                        DbErr::RecordNotFound("item value count returned no row".to_owned())
                    })?
                    .try_get::<i64>("", "total_record_count")?,
            )
        } else {
            None
        };

        let mut page_values = values;
        let direction = if query.descending { "DESC" } else { "ASC" };
        let order = match query.order {
            ItemValueOrder::CleanValue => format!("clean_value {direction}, item_value_id"),
            ItemValueOrder::Random => "random(), item_value_id".to_owned(),
        };
        let mut page_sql = format!(
            "{cte} SELECT item_value_id, value, item_count \
             FROM values ORDER BY {order}"
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
        let values = self
            .database
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                page_sql,
                page_values,
            ))
            .await?
            .into_iter()
            .map(|row| {
                Ok(ItemValueInfo {
                    id: row.try_get("", "item_value_id")?,
                    value: row.try_get("", "value")?,
                    item_count: u64::try_from(row.try_get::<i64>("", "item_count")?)
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, DbErr>>()?;
        Ok(ItemValuePage {
            total_record_count: count
                .map(|count| u64::try_from(count).unwrap_or_default())
                .unwrap_or_else(|| u64::try_from(values.len()).unwrap_or(u64::MAX)),
            values,
            start_index: query.start_index,
        })
    }
}

fn total_count_enabled(query: &ItemValueQuery) -> bool {
    query.enable_total_record_count.unwrap_or(true)
}

fn item_values_cte(
    value_type: item_value::ItemValueType,
    query: &ItemValueQuery,
) -> (String, Vec<SeaValue>) {
    let mut values = vec![item_value_type_code(value_type).into()];
    let mut sql = String::from(
        "WITH linked AS (\
             SELECT value.item_value_id, value.value, value.clean_value, item.id AS item_id \
             FROM jellyfin.item_values AS value \
             JOIN jellyfin.item_value_map AS map ON map.item_value_id = value.item_value_id \
             JOIN jellyfin.base_items AS item ON item.id = map.item_id \
             WHERE value.type = $1 \
               AND item.item_type <> 'PLACEHOLDER'",
    );
    append_item_filters(&mut sql, &mut values, query);
    append_value_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), values AS (\
             SELECT item_value_id, value, clean_value, COUNT(DISTINCT item_id)::bigint AS item_count \
             FROM linked \
             GROUP BY item_value_id, value, clean_value\
         )",
    );
    (sql, values)
}

const fn item_value_type_code(value_type: item_value::ItemValueType) -> i16 {
    match value_type {
        item_value::ItemValueType::Artist => 0,
        item_value::ItemValueType::AlbumArtist => 1,
        item_value::ItemValueType::Genre => 2,
        item_value::ItemValueType::Studios => 3,
        item_value::ItemValueType::Tags => 4,
        item_value::ItemValueType::InheritedTags => 6,
    }
}

fn append_item_filters(sql: &mut String, values: &mut Vec<SeaValue>, query: &ItemValueQuery) {
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
    if let Some(is_favorite) = query.is_favorite
        && query.by_name_item_type.is_none()
    {
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

fn append_value_filters(sql: &mut String, values: &mut Vec<SeaValue>, query: &ItemValueQuery) {
    if let Some(is_favorite) = query.is_favorite
        && let Some(item_type) = query.by_name_item_type.as_deref()
    {
        let Some(user_id) = query.user_id else {
            return;
        };
        push_bind(
            sql,
            values,
            item_type.to_owned(),
            " AND (EXISTS (
                SELECT 1 FROM jellyfin.base_items AS by_name
                JOIN jellyfin.user_data AS data ON data.item_id = by_name.id
                WHERE by_name.item_type = ",
        );
        sql.push_str(" AND by_name.clean_name = value.clean_value");
        push_bind(sql, values, user_id, " AND data.user_id = ");
        sql.push_str(" AND data.is_favorite = true) = ");
        values.push(is_favorite.into());
        sql.push('$');
        sql.push_str(&values.len().to_string());
        sql.push(')');
    }
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
            " AND value.clean_value ILIKE ",
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
            " AND value.clean_value ILIKE ",
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
            " AND value.clean_value >= ",
        );
    }
    if let Some(name) = query
        .name_less_than
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        push_bind(sql, values, name.clean_value(), " AND value.clean_value < ");
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

async fn upsert_on<C>(
    connection: &C,
    value_type: item_value::ItemValueType,
    value: &str,
) -> Result<item_value::Model, ItemValueError>
where
    C: ConnectionTrait,
{
    let value = validate_value(value)?;
    let clean_value = value.clean_value();
    if clean_value.is_empty() {
        return Err(ItemValueError::InvalidValue);
    }
    let active = item_value::ActiveModel {
        item_value_id: Set(Uuid::new_v4()),
        value_type: Set(value_type),
        value: Set(value.to_owned()),
        clean_value: Set(clean_value),
    };
    Ok(item_value::Entity::insert(active)
        .on_conflict(
            OnConflict::columns([
                item_value::Column::ValueType,
                item_value::Column::CleanValue,
            ])
            .update_column(item_value::Column::CleanValue)
            .to_owned(),
        )
        .exec_with_returning(connection)
        .await?)
}

fn validate_value(value: &str) -> Result<&str, ItemValueError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ItemValueError::InvalidValue)
    } else {
        Ok(value)
    }
}
