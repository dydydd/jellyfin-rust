use std::{collections::HashMap, fmt::Write as _};

use jellyfin_extensions::StringExtensions;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, Statement, TransactionTrait, Value as SeaValue, sea_query::OnConflict,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    base_item_repository::{BaseItemQuery, append_is_played_filter, policy_filter_sql},
    entities::{base_item, item_value, item_value_map},
    item_types::expand_item_type_aliases,
};

#[derive(Debug, Error)]
pub enum ItemValueError {
    #[error("item value cannot be empty")]
    InvalidValue,
    #[error("base item was not found")]
    ItemNotFound,
    #[error(transparent)]
    Database(#[from] DbErr),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemValueQuery {
    pub ids: Vec<Uuid>,
    pub parent_id: Option<Uuid>,
    pub recursive: bool,
    pub search_term: Option<String>,
    pub include_item_types: Vec<String>,
    pub exclude_item_types: Vec<String>,
    /// Internal source-type exclusions used only to discover item-by-name values.
    ///
    /// Unlike API-facing `ExcludeItemTypes`, these must not narrow the count
    /// buckets after a value has been selected.
    #[doc(hidden)]
    pub discovery_exclude_item_types: Vec<String>,
    pub media_types: Vec<String>,
    pub is_movie: Option<bool>,
    pub is_series: Option<bool>,
    pub is_news: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_sports: Option<bool>,
    pub is_airing: Option<bool>,
    pub is_favorite: Option<bool>,
    pub is_liked: Option<bool>,
    pub is_favorite_or_liked: Option<bool>,
    pub is_played: Option<bool>,
    pub user_id: Option<Uuid>,
    pub by_name_item_type: Option<String>,
    pub genres: Vec<String>,
    pub genre_ids: Vec<Uuid>,
    pub official_ratings: Vec<String>,
    pub tags: Vec<String>,
    pub years: Vec<i32>,
    pub studios: Vec<String>,
    pub studio_ids: Vec<Uuid>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_starts_with: Option<String>,
    pub name_less_than: Option<String>,
    pub start_index: u64,
    pub limit: Option<u64>,
    pub order: ItemValueOrder,
    pub descending: bool,
    pub enable_total_record_count: Option<bool>,
    pub access_policy: BaseItemQuery,
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
    pub counts: ItemValueCounts,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemValueCounts {
    pub album_count: u64,
    pub artist_count: u64,
    pub episode_count: u64,
    pub movie_count: u64,
    pub music_video_count: u64,
    pub program_count: u64,
    pub series_count: u64,
    pub song_count: u64,
    pub trailer_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemValuePair {
    pub id: Uuid,
    pub value: String,
}

/// One persisted item-by-name entity required by the current genre credits.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct ItemByNameValue {
    pub item_type: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemValuePage {
    pub values: Vec<ItemValueInfo>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, FromQueryResult)]
struct GenreIdRow {
    item_value_id: Uuid,
}

/// PostgreSQL-backed normalized item values and their many-to-many base-item
/// associations.
#[derive(Clone)]
pub struct ItemValueRepository {
    database: crate::SharedDatabase,
}

impl ItemValueRepository {
    #[must_use]
    pub fn new(database: impl Into<crate::SharedDatabase>) -> Self {
        Self {
            database: database.into(),
        }
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
        upsert_on(self.database.as_ref(), value_type, value).await
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
            .one(self.database.as_ref())
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
            .one(self.database.as_ref())
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
            .one(self.database.as_ref())
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
            .all(self.database.as_ref())
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
            .all(self.database.as_ref())
            .await?)
    }

    /// Loads normalized values of one type for many items in one query.
    ///
    /// Values are ordered by clean name, matching the single-item projection.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn values_for_items(
        &self,
        item_ids: &[Uuid],
        value_type: item_value::ItemValueType,
    ) -> Result<HashMap<Uuid, Vec<String>>, ItemValueError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut values = vec![item_value_type_code(value_type).into()];
        let placeholders = (2..=item_ids.len() + 1)
            .map(|index| format!("${index}::uuid"))
            .collect::<Vec<_>>()
            .join(", ");
        values.extend(item_ids.iter().copied().map(SeaValue::from));
        let rows = self
            .database
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "SELECT map.item_id, value.value \
                     FROM jellyfin.item_value_map AS map \
                     INNER JOIN jellyfin.item_values AS value \
                       ON value.item_value_id = map.item_value_id \
                     WHERE value.type = $1 \
                       AND map.item_id IN ({placeholders}) \
                     ORDER BY map.item_id, value.clean_value, value.item_value_id"
                ),
                values,
            ))
            .await?;
        let mut result = HashMap::<Uuid, Vec<String>>::new();
        for row in rows {
            let item_id = row.try_get("", "item_id")?;
            let value = row.try_get("", "value")?;
            result.entry(item_id).or_default().push(value);
        }
        Ok(result)
    }

    /// Loads normalized value identifiers and display names of one type for
    /// many items in one query.
    ///
    /// Values are ordered by clean name and stable identifier. This is used by
    /// API projections whose official DTO shape contains both `Name` and `Id`.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn value_pairs_for_items(
        &self,
        item_ids: &[Uuid],
        value_type: item_value::ItemValueType,
    ) -> Result<HashMap<Uuid, Vec<ItemValuePair>>, ItemValueError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut values = vec![item_value_type_code(value_type).into()];
        let placeholders = (2..=item_ids.len() + 1)
            .map(|index| format!("${index}::uuid"))
            .collect::<Vec<_>>()
            .join(", ");
        values.extend(item_ids.iter().copied().map(SeaValue::from));
        let rows = self
            .database
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "SELECT map.item_id, value.item_value_id, value.value \
                     FROM jellyfin.item_value_map AS map \
                     INNER JOIN jellyfin.item_values AS value \
                       ON value.item_value_id = map.item_value_id \
                     WHERE value.type = $1 \
                       AND map.item_id IN ({placeholders}) \
                     ORDER BY map.item_id, value.clean_value, value.item_value_id"
                ),
                values,
            ))
            .await?;
        let mut result = HashMap::<Uuid, Vec<ItemValuePair>>::new();
        for row in rows {
            let item_id = row.try_get("", "item_id")?;
            let pair = ItemValuePair {
                id: row.try_get("", "item_value_id")?,
                value: row.try_get("", "value")?,
            };
            result.entry(item_id).or_default().push(pair);
        }
        Ok(result)
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
            .all(self.database.as_ref())
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
            .all(self.database.as_ref())
            .await?)
    }

    /// Selects random audio items sharing at least one normalized genre.
    ///
    /// `PostgreSQL` performs the set intersection, deduplication, randomization,
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
        access_policy: &BaseItemQuery,
    ) -> Result<Vec<base_item::Model>, ItemValueError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let audio_item_types = expand_item_type_aliases(&["Audio".to_owned()]);
        let mut values = genre_ids
            .iter()
            .copied()
            .map(SeaValue::from)
            .collect::<Vec<_>>();
        let genre_placeholders = (1..=genre_ids.len())
            .map(|index| format!("${index}::uuid"))
            .collect::<Vec<_>>()
            .join(", ");
        let item_type_placeholders = ((genre_ids.len() + 1)
            ..=(genre_ids.len() + audio_item_types.len()))
            .map(|index| format!("${index}::text"))
            .collect::<Vec<_>>()
            .join(", ");
        values.extend(audio_item_types.into_iter().map(SeaValue::from));
        values.push(limit.into());
        let mut sql = format!(
            "SELECT item.* \
                 FROM jellyfin.base_items AS item \
                 WHERE item.item_type IN ({item_type_placeholders}) \
                   AND item.is_virtual_item = false"
        );
        if !genre_ids.is_empty() {
            let _ = write!(
                sql,
                " AND EXISTS ( \
                       SELECT 1 FROM jellyfin.item_value_map AS map \
                       INNER JOIN jellyfin.item_values AS value \
                         ON value.item_value_id = map.item_value_id \
                       WHERE map.item_id = item.id \
                         AND value.type = 2 \
                         AND value.item_value_id IN ({genre_placeholders}) \
                    )"
            );
        }
        if let Some(condition) = policy_filter_sql("item", access_policy) {
            sql.push_str(" AND (");
            sql.push_str(&condition);
            sql.push(')');
        }
        let _ = write!(sql, " ORDER BY random() LIMIT ${}::bigint", values.len());
        Ok(
            base_item::Model::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .all(self.database.as_ref())
            .await?,
        )
    }

    /// Loads the distinct genres attached to a folder or any visible audio descendant.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn genre_ids_for_folder_audio(
        &self,
        folder_id: Uuid,
        access_policy: &BaseItemQuery,
    ) -> Result<Vec<Uuid>, ItemValueError> {
        let audio_item_types = expand_item_type_aliases(&["Audio".to_owned()]);
        let mut values = vec![folder_id.into()];
        let item_type_placeholders = (2..=(audio_item_types.len() + 1))
            .map(|index| format!("${index}::text"))
            .collect::<Vec<_>>()
            .join(", ");
        values.extend(audio_item_types.into_iter().map(SeaValue::from));
        let mut sql = format!(
            "SELECT DISTINCT value.item_value_id \
             FROM jellyfin.item_values AS value \
             JOIN jellyfin.item_value_map AS map ON map.item_value_id = value.item_value_id \
             WHERE value.type = 2 \
               AND (map.item_id = $1::uuid OR EXISTS ( \
                   SELECT 1 \
                   FROM jellyfin.base_items AS audio \
                   JOIN jellyfin.ancestor_ids AS closure ON closure.item_id = audio.id \
                   WHERE audio.id = map.item_id \
                     AND closure.parent_item_id = $1::uuid \
                     AND audio.item_type IN ({item_type_placeholders}) \
                     AND audio.is_virtual_item = false"
        );
        if let Some(condition) = policy_filter_sql("audio", access_policy) {
            sql.push_str(" AND (");
            sql.push_str(&condition);
            sql.push(')');
        }
        sql.push_str(")) ORDER BY value.item_value_id");
        Ok(
            GenreIdRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .all(self.database.as_ref())
            .await?
            .into_iter()
            .map(|row| row.item_value_id)
            .collect(),
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
            .exec(self.database.as_ref())
            .await?;
        Ok(())
    }

    /// Lists one keyset page of generic and music genre entities required after a library scan.
    ///
    /// Official Jellyfin builds the two sets independently: music item types
    /// contribute to `MusicGenre`, while every other type contributes to
    /// `Genre`. A normalized value used by both sets therefore yields both
    /// entity kinds. The minimum display value matches the official grouped
    /// item-value query and keeps deterministic paths and identifiers.
    ///
    /// # Errors
    ///
    /// Returns a database error when the set-based query fails.
    pub async fn required_genre_entities_page(
        &self,
        after: Option<&ItemByNameValue>,
        limit: usize,
    ) -> Result<Vec<ItemByNameValue>, ItemValueError> {
        const MAX_PAGE_SIZE: usize = 512;
        const MUSIC_TYPES: &str = "(\
            'Audio', 'MediaBrowser.Controller.Entities.Audio.Audio', \
            'MusicVideo', 'MediaBrowser.Controller.Entities.MusicVideo', \
            'MusicAlbum', 'MediaBrowser.Controller.Entities.Audio.MusicAlbum', \
            'MusicArtist', 'MediaBrowser.Controller.Entities.Audio.MusicArtist'\
        )";
        let mut sql = format!(
            "WITH names AS (\
                     SELECT value.clean_value, MIN(value.value) AS name, \
                            bool_or(item.item_type IN {MUSIC_TYPES}) AS has_music, \
                            bool_or(item.item_type NOT IN {MUSIC_TYPES}) AS has_generic \
                     FROM jellyfin.item_values AS value \
                     JOIN jellyfin.item_value_map AS map \
                       ON map.item_value_id = value.item_value_id \
                     JOIN jellyfin.base_items AS item ON item.id = map.item_id \
                     WHERE value.type = 2 \
                     GROUP BY value.clean_value\
                 ) \
                 , required AS (\
                     SELECT 'Genre'::text AS item_type, names.name FROM names \
                     WHERE has_generic \
                   AND NOT EXISTS (\
                       SELECT 1 FROM jellyfin.base_items AS existing \
                       WHERE existing.clean_name = names.clean_value \
                         AND existing.item_type IN (\
                             'Genre', 'MediaBrowser.Controller.Entities.Genre')) \
                     UNION ALL \
                     SELECT 'MusicGenre'::text AS item_type, names.name FROM names \
                     WHERE has_music \
                   AND NOT EXISTS (\
                       SELECT 1 FROM jellyfin.base_items AS existing \
                       WHERE existing.clean_name = names.clean_value \
                         AND existing.item_type IN (\
                                 'MusicGenre', \
                                 'MediaBrowser.Controller.Entities.Audio.MusicGenre'))\
                 ) \
                 SELECT item_type, name FROM required"
        );
        let mut values = Vec::with_capacity(3);
        if let Some(after) = after {
            push_bind(
                &mut sql,
                &mut values,
                after.item_type.clone(),
                " WHERE (item_type, name) > (",
            );
            push_bind(&mut sql, &mut values, after.name.clone(), ", ");
            sql.push(')');
        }
        sql.push_str(" ORDER BY item_type, name LIMIT ");
        push_bind(
            &mut sql,
            &mut values,
            i64::try_from(limit.clamp(1, MAX_PAGE_SIZE)).unwrap_or(512),
            "",
        );
        let statement = Statement::from_sql_and_values(DbBackend::Postgres, sql, values);
        Ok(ItemByNameValue::find_by_statement(statement)
            .all(self.database.as_ref())
            .await?)
    }

    /// Lists one keyset page of persisted Studio entities required after a library scan.
    ///
    /// The source is the normalized set of Studio values still attached to a
    /// base item. Existing canonical or legacy CLR Studio rows suppress a new
    /// entity, preserving any metadata or images already stored on that row.
    ///
    /// # Errors
    ///
    /// Returns a database error when the set-based query fails.
    pub async fn required_studio_entities_page(
        &self,
        after: Option<&ItemByNameValue>,
        limit: usize,
    ) -> Result<Vec<ItemByNameValue>, ItemValueError> {
        const MAX_PAGE_SIZE: usize = 512;
        let mut sql = String::from(
            "WITH names AS (\
                 SELECT value.clean_value, MIN(value.value) AS name \
                 FROM jellyfin.item_values AS value \
                 JOIN jellyfin.item_value_map AS map \
                   ON map.item_value_id = value.item_value_id \
                 JOIN jellyfin.base_items AS item ON item.id = map.item_id \
                 WHERE value.type = 3 \
                 GROUP BY value.clean_value\
             ), required AS (\
                 SELECT 'Studio'::text AS item_type, names.name FROM names \
                 WHERE NOT EXISTS (\
                     SELECT 1 FROM jellyfin.base_items AS existing \
                     WHERE existing.clean_name = names.clean_value \
                       AND existing.item_type IN (\
                           'Studio', 'MediaBrowser.Controller.Entities.Studio'))\
             ) \
             SELECT item_type, name FROM required",
        );
        let mut values = Vec::with_capacity(3);
        if let Some(after) = after {
            push_bind(
                &mut sql,
                &mut values,
                after.item_type.clone(),
                " WHERE (item_type, name) > (",
            );
            push_bind(&mut sql, &mut values, after.name.clone(), ", ");
            sql.push(')');
        }
        sql.push_str(" ORDER BY item_type, name LIMIT ");
        push_bind(
            &mut sql,
            &mut values,
            i64::try_from(limit.clamp(1, MAX_PAGE_SIZE)).unwrap_or(512),
            "",
        );
        let statement = Statement::from_sql_and_values(DbBackend::Postgres, sql, values);
        Ok(ItemByNameValue::find_by_statement(statement)
            .all(self.database.as_ref())
            .await?)
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
        self.query_value_page(cte, values, "values", query).await
    }

    /// Lists persisted item-by-name entities attached to the filtered values.
    ///
    /// Entities sharing a presentation key are folded before counting,
    /// ordering, and paging, with the smallest UUID representing the group.
    /// This mirrors Jellyfin's item-by-name repository while retaining the
    /// already set-based value-count aggregation.
    ///
    /// # Errors
    ///
    /// Returns a database error when the entity or value query fails.
    pub async fn query_persisted_item_by_name_values(
        &self,
        value_type: item_value::ItemValueType,
        item_type: &str,
        query: &ItemValueQuery,
    ) -> Result<ItemValuePage, ItemValueError> {
        let (mut cte, mut values) = item_values_cte(value_type, query);
        cte.push_str(
            ", by_name_candidates AS (\
                 SELECT by_name.id AS item_value_id, \
                        COALESCE(by_name.name, value.value) AS value, \
                        COALESCE(\
                            by_name.clean_name, by_name.sort_name, by_name.name, value.clean_value\
                        ) AS clean_value, \
                        by_name.presentation_unique_key, \
                        value.item_count, value.album_count, value.artist_count, \
                        value.episode_count, value.movie_count, value.music_video_count, \
                        value.program_count, value.series_count, value.song_count, \
                        value.trailer_count \
                 FROM values AS value \
                 JOIN jellyfin.base_items AS by_name \
                   ON by_name.clean_name = value.clean_value \
                 WHERE TRUE",
        );
        let item_types = expand_item_type_aliases(&[item_type.to_owned()]);
        append_string_list_filter(
            &mut cte,
            &mut values,
            "by_name.item_type",
            &item_types,
            false,
        );
        append_persisted_by_name_user_data_filters(&mut cte, &mut values, query, "by_name");
        append_persisted_by_name_metadata_filters(&mut cte, &mut values, query, "by_name");
        cte.push_str(
            "), representatives AS (\
                 SELECT DISTINCT ON (presentation_unique_key) \
                        item_value_id, value, clean_value, item_count, album_count, \
                        artist_count, episode_count, movie_count, music_video_count, \
                        program_count, series_count, song_count, trailer_count \
                 FROM by_name_candidates \
                 ORDER BY presentation_unique_key, item_value_id\
             )",
        );
        self.query_value_page(cte, values, "representatives", query)
            .await
    }

    async fn query_value_page(
        &self,
        cte: String,
        values: Vec<SeaValue>,
        source: &str,
        query: &ItemValueQuery,
    ) -> Result<ItemValuePage, ItemValueError> {
        let count = if total_count_enabled(query) {
            Some(
                self.database
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        format!("{cte} SELECT COUNT(*) AS total_record_count FROM {source}"),
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
            "{cte} SELECT item_value_id, value, item_count, album_count, artist_count, \
                    episode_count, movie_count, music_video_count, program_count, \
                    series_count, song_count, trailer_count \
             FROM {source} ORDER BY {order}"
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
                let count = |column| -> Result<u64, DbErr> {
                    Ok(u64::try_from(row.try_get::<i64>("", column)?).unwrap_or_default())
                };
                Ok(ItemValueInfo {
                    id: row.try_get("", "item_value_id")?,
                    value: row.try_get("", "value")?,
                    item_count: u64::try_from(row.try_get::<i64>("", "item_count")?)
                        .unwrap_or_default(),
                    counts: ItemValueCounts {
                        album_count: count("album_count")?,
                        artist_count: count("artist_count")?,
                        episode_count: count("episode_count")?,
                        movie_count: count("movie_count")?,
                        music_video_count: count("music_video_count")?,
                        program_count: count("program_count")?,
                        series_count: count("series_count")?,
                        song_count: count("song_count")?,
                        trailer_count: count("trailer_count")?,
                    },
                })
            })
            .collect::<Result<Vec<_>, DbErr>>()?;
        Ok(ItemValuePage {
            total_record_count: count.map_or_else(
                || u64::try_from(values.len()).unwrap_or(u64::MAX),
                |count| u64::try_from(count).unwrap_or_default(),
            ),
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
    let inherits_to_episodes = matches!(
        value_type,
        item_value::ItemValueType::Genre | item_value::ItemValueType::Studios
    );
    let mut values = vec![item_value_type_code(value_type).into()];
    let mut sql = String::from(
        "WITH matching AS (\
             SELECT value.item_value_id, value.value, value.clean_value, \
                    item.id AS item_id, item.item_type \
             FROM jellyfin.item_values AS value \
             JOIN jellyfin.item_value_map AS map ON map.item_value_id = value.item_value_id \
             JOIN jellyfin.base_items AS item ON item.id = map.item_id \
             WHERE value.type = $1 \
               AND item.item_type <> 'PLACEHOLDER' \
               AND item.primary_version_id IS NULL \
               AND (item.data ->> 'OwnerId' IS NULL \
                    OR item.data ->> 'ExtraType' IS NOT NULL)",
    );
    append_item_filters(&mut sql, &mut values, query);
    if let Some(condition) = policy_filter_sql("item", &query.access_policy) {
        sql.push_str(" AND (");
        sql.push_str(&condition);
        sql.push(')');
    }
    append_value_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), selected_values AS (\
             SELECT DISTINCT item_value_id, value, clean_value FROM matching\
         ), scoped AS (\
             SELECT selected.item_value_id, selected.value, selected.clean_value, \
                    item.id AS item_id, item.item_type \
             FROM selected_values AS selected \
             JOIN jellyfin.item_value_map AS map \
               ON map.item_value_id = selected.item_value_id \
             JOIN jellyfin.base_items AS item ON item.id = map.item_id \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.primary_version_id IS NULL \
               AND (item.data ->> 'OwnerId' IS NULL \
                    OR item.data ->> 'ExtraType' IS NOT NULL)",
    );
    append_count_scope_filters(&mut sql, &mut values, query, "item");
    if let Some(condition) = policy_filter_sql("item", &query.access_policy) {
        sql.push_str(" AND (");
        sql.push_str(&condition);
        sql.push(')');
    }
    sql.push(')');
    if inherits_to_episodes {
        append_inherited_episode_counts_cte(&mut sql, &mut values, query);
    } else {
        sql.push_str(
            ", counted AS (\
                 SELECT item_value_id, value, clean_value, item_id, item_type FROM scoped\
             )",
        );
    }
    append_item_value_count_buckets_cte(&mut sql);
    (sql, values)
}

fn append_inherited_episode_counts_cte(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &ItemValueQuery,
) {
    sql.push_str(
        ", inherited_episodes AS (\
             SELECT tagged.item_value_id, tagged.value, tagged.clean_value, \
                    episode.id AS item_id, episode.item_type \
             FROM scoped AS tagged \
             JOIN jellyfin.base_items AS episode \
               ON episode.series_id = tagged.item_id \
             WHERE tagged.item_type IN (\
                     'Series', 'MediaBrowser.Controller.Entities.TV.Series'\
                 ) \
               AND episode.item_type IN (\
                     'Episode', 'MediaBrowser.Controller.Entities.TV.Episode'\
                 ) \
               AND episode.primary_version_id IS NULL \
               AND (episode.data ->> 'OwnerId' IS NULL \
                    OR episode.data ->> 'ExtraType' IS NOT NULL)",
    );
    append_count_scope_filters(sql, values, query, "episode");
    if let Some(condition) = policy_filter_sql("episode", &query.access_policy) {
        sql.push_str(" AND (");
        sql.push_str(&condition);
        sql.push(')');
    }
    sql.push_str(
        "), counted AS (\
             SELECT item_value_id, value, clean_value, item_id, item_type FROM scoped \
             UNION ALL \
             SELECT item_value_id, value, clean_value, item_id, item_type \
             FROM inherited_episodes\
         )",
    );
}

fn append_item_value_count_buckets_cte(sql: &mut String) {
    sql.push_str(
        ", values AS (\
             SELECT item_value_id, value, clean_value, \
                    COUNT(DISTINCT item_id)::bigint AS item_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'MusicAlbum', 'MediaBrowser.Controller.Entities.Audio.MusicAlbum'\
                    ))::bigint AS album_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'MusicArtist', 'MediaBrowser.Controller.Entities.Audio.MusicArtist'\
                    ))::bigint AS artist_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'Episode', 'MediaBrowser.Controller.Entities.TV.Episode'\
                    ))::bigint AS episode_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'Movie', 'MediaBrowser.Controller.Entities.Movies.Movie'\
                    ))::bigint AS movie_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'MusicVideo', 'MediaBrowser.Controller.Entities.MusicVideo'\
                    ))::bigint AS music_video_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type = 'Program')::bigint AS program_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'Series', 'MediaBrowser.Controller.Entities.TV.Series'\
                    ))::bigint AS series_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'Audio', 'MediaBrowser.Controller.Entities.Audio.Audio'\
                    ))::bigint AS song_count, \
                    COUNT(DISTINCT item_id) FILTER (WHERE item_type IN (\
                        'Trailer', 'MediaBrowser.Controller.Entities.Trailer'\
                    ))::bigint AS trailer_count \
             FROM counted \
             GROUP BY item_value_id, value, clean_value\
         )",
    );
}

fn append_count_scope_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &ItemValueQuery,
    table: &str,
) {
    if !query.ids.is_empty() {
        sql.push_str(" AND ");
        sql.push_str(table);
        sql.push_str(".id IN (");
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
            values.push(parent_id.into());
            sql.push_str(" AND ");
            sql.push_str(table);
            sql.push_str(
                ".id IN (SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
                  WHERE closure.parent_item_id = $",
            );
            sql.push_str(&values.len().to_string());
            sql.push(')');
        } else {
            values.push(parent_id.into());
            sql.push_str(" AND ");
            sql.push_str(table);
            sql.push_str(".parent_id = $");
            sql.push_str(&values.len().to_string());
        }
    }
    let item_type = format!("{table}.item_type");
    let exclude_item_types = expand_item_type_aliases(&query.exclude_item_types);
    append_string_list_filter(sql, values, &item_type, &exclude_item_types, true);
    let media_type = format!("{table}.media_type");
    append_string_list_filter(sql, values, &media_type, &query.media_types, false);
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
    let include_item_types = expand_item_type_aliases(&query.include_item_types);
    append_string_list_filter(sql, values, "item.item_type", &include_item_types, false);
    let exclude_item_types = expand_item_type_aliases(&query.exclude_item_types);
    append_string_list_filter(sql, values, "item.item_type", &exclude_item_types, true);
    let discovery_exclude_item_types =
        expand_item_type_aliases(&query.discovery_exclude_item_types);
    append_string_list_filter(
        sql,
        values,
        "item.item_type",
        &discovery_exclude_item_types,
        true,
    );
    append_string_list_filter(sql, values, "item.media_type", &query.media_types, false);
    append_media_class_filter(sql, query.is_movie, "IsMovie", &["Movie", "Trailer"]);
    append_media_class_filter(sql, query.is_series, "IsSeries", &["Series"]);
    append_tag_class_filter(sql, query.is_sports, "sports");
    append_tag_class_filter(sql, query.is_news, "news");
    append_tag_class_filter(sql, query.is_kids, "kids");
    append_airing_filter(sql, query.is_airing);
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

fn append_airing_filter(sql: &mut String, expected: Option<bool>) {
    let Some(expected) = expected else {
        return;
    };
    let start_date = "NULLIF(item.data ->> 'StartDate', '')::timestamptz";
    let end_date = "NULLIF(item.data ->> 'EndDate', '')::timestamptz";
    sql.push_str(" AND (");
    if expected {
        let _ = write!(
            sql,
            "{start_date} <= CURRENT_TIMESTAMP AND {end_date} >= CURRENT_TIMESTAMP"
        );
    } else {
        let _ = write!(
            sql,
            "{start_date} > CURRENT_TIMESTAMP OR {end_date} < CURRENT_TIMESTAMP"
        );
    }
    sql.push(')');
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
    if let Some(expected) = query.is_favorite {
        append_by_name_user_data_filter(sql, values, query, "data.is_favorite = true", expected);
    }
    if let Some(expected) = query.is_favorite_or_liked {
        // Official Jellyfin currently applies this item-by-name filter to the
        // favorite flag only, despite the legacy parameter name.
        append_by_name_user_data_filter(sql, values, query, "data.is_favorite = true", expected);
    }
    if let Some(expected) = query.is_liked {
        append_by_name_user_data_filter(sql, values, query, "data.likes = true", expected);
    }
    if let Some(is_played) = query.is_played
        && let (Some(item_type), Some(user_id)) =
            (query.by_name_item_type.as_deref(), query.user_id)
    {
        sql.push_str(
            " AND EXISTS (
                SELECT 1 FROM jellyfin.base_items AS by_name
                WHERE TRUE",
        );
        append_by_name_item_type_filter(sql, values, item_type, "by_name");
        sql.push_str(" AND by_name.clean_name = value.clean_value AND");
        append_is_played_filter(sql, values, user_id, "by_name", is_played);
        sql.push(')');
    }
    append_by_name_metadata_filters(sql, values, query);
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

fn append_by_name_metadata_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &ItemValueQuery,
) {
    let Some(item_type) = query.by_name_item_type.as_deref() else {
        return;
    };
    if query.genres.is_empty()
        && query.genre_ids.is_empty()
        && query.official_ratings.is_empty()
        && query.tags.is_empty()
        && query.years.is_empty()
        && query.studios.is_empty()
        && query.studio_ids.is_empty()
    {
        return;
    }

    sql.push_str(
        " AND EXISTS (\
            SELECT 1 FROM jellyfin.base_items AS by_name \
            WHERE TRUE",
    );
    append_by_name_item_type_filter(sql, values, item_type, "by_name");
    sql.push_str(" AND by_name.clean_name = value.clean_value");
    append_persisted_by_name_metadata_filters(sql, values, query, "by_name");
    sql.push(')');
}

fn append_persisted_by_name_metadata_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &ItemValueQuery,
    table: &str,
) {
    append_by_name_item_value_names(
        sql,
        values,
        table,
        item_value::ItemValueType::Genre,
        &query.genres,
    );
    append_by_name_item_value_reference_ids(
        sql,
        values,
        table,
        item_value::ItemValueType::Genre,
        &query.genre_ids,
    );
    append_by_name_item_value_names(
        sql,
        values,
        table,
        item_value::ItemValueType::Tags,
        &query.tags,
    );
    append_by_name_item_value_names(
        sql,
        values,
        table,
        item_value::ItemValueType::Studios,
        &query.studios,
    );
    append_by_name_item_value_reference_ids(
        sql,
        values,
        table,
        item_value::ItemValueType::Studios,
        &query.studio_ids,
    );
    if !query.years.is_empty() {
        let _ = write!(sql, " AND {table}.production_year IN (");
        append_bind_values(sql, values, query.years.iter().copied());
        sql.push(')');
    }
    if !query.official_ratings.is_empty() {
        let _ = write!(sql, " AND ({table}.official_rating IN (");
        append_bind_values(sql, values, query.official_ratings.iter().cloned());
        let _ = write!(
            sql,
            ") OR EXISTS (\
                SELECT 1 FROM jellyfin.ancestor_ids AS rating_closure \
                JOIN jellyfin.base_items AS rating_descendant \
                  ON rating_descendant.id = rating_closure.item_id \
                WHERE rating_closure.parent_item_id = {table}.id \
                  AND rating_descendant.official_rating IN ("
        );
        append_bind_values(sql, values, query.official_ratings.iter().cloned());
        let _ = write!(
            sql,
            ")) OR EXISTS (\
                SELECT 1 FROM jellyfin.linked_children AS rating_link \
                JOIN jellyfin.base_items AS rating_child \
                  ON rating_child.id = rating_link.child_id \
                WHERE rating_link.parent_id = {table}.id \
                  AND rating_child.official_rating IN ("
        );
        append_bind_values(sql, values, query.official_ratings.iter().cloned());
        sql.push_str(")))");
    }
}

fn append_by_name_item_value_names(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    table: &str,
    value_type: item_value::ItemValueType,
    names: &[String],
) {
    if names.is_empty() {
        return;
    }
    let clean_names = names.iter().map(|name| name.clean_value());
    let _ = write!(
        sql,
        " AND EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS metadata_map \
            JOIN jellyfin.item_values AS metadata_value \
              ON metadata_value.item_value_id = metadata_map.item_value_id \
            WHERE metadata_map.item_id = {table}.id \
              AND metadata_value.type = {} \
              AND metadata_value.clean_value IN (",
        item_value_type_code(value_type)
    );
    append_bind_values(sql, values, clean_names);
    sql.push_str("))");
}

fn append_by_name_item_value_reference_ids(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    table: &str,
    value_type: item_value::ItemValueType,
    reference_ids: &[Uuid],
) {
    if reference_ids.is_empty() {
        return;
    }
    let _ = write!(
        sql,
        " AND EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS metadata_map \
            JOIN jellyfin.item_values AS metadata_value \
              ON metadata_value.item_value_id = metadata_map.item_value_id \
            WHERE metadata_map.item_id = {table}.id \
              AND metadata_value.type = {} \
              AND metadata_value.clean_value IN (\
                  SELECT referenced.clean_name FROM jellyfin.base_items AS referenced \
                  WHERE referenced.id IN (",
        item_value_type_code(value_type)
    );
    append_bind_values(sql, values, reference_ids.iter().copied());
    sql.push_str(")))");
}

fn append_bind_values<T>(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    items: impl IntoIterator<Item = T>,
) where
    T: Into<SeaValue>,
{
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        values.push(item.into());
        sql.push('$');
        sql.push_str(&values.len().to_string());
    }
}

fn append_by_name_user_data_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &ItemValueQuery,
    predicate: &str,
    expected: bool,
) {
    let (Some(item_type), Some(user_id)) = (query.by_name_item_type.as_deref(), query.user_id)
    else {
        return;
    };
    sql.push_str(
        " AND EXISTS (
            SELECT 1 FROM jellyfin.base_items AS by_name
            WHERE TRUE",
    );
    append_by_name_item_type_filter(sql, values, item_type, "by_name");
    sql.push_str(" AND by_name.clean_name = value.clean_value AND (EXISTS (");
    push_bind(
        sql,
        values,
        user_id,
        "SELECT 1 FROM jellyfin.user_data AS data
         WHERE data.item_id = by_name.id AND data.user_id = ",
    );
    sql.push_str(" AND ");
    sql.push_str(predicate);
    sql.push_str(")) = ");
    values.push(expected.into());
    sql.push('$');
    sql.push_str(&values.len().to_string());
    sql.push(')');
}

fn append_persisted_by_name_user_data_filters(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    query: &ItemValueQuery,
    table: &str,
) {
    let Some(user_id) = query.user_id else {
        return;
    };
    for (expected, predicate) in [
        (query.is_favorite, "data.is_favorite = true"),
        // Official Jellyfin currently treats `IsFavoriteOrLiked` as the
        // favorite flag for item-by-name queries.
        (query.is_favorite_or_liked, "data.is_favorite = true"),
        (query.is_liked, "data.likes = true"),
    ] {
        let Some(expected) = expected else {
            continue;
        };
        let _ = write!(
            sql,
            " AND (EXISTS (\
                 SELECT 1 FROM jellyfin.user_data AS data \
                 WHERE data.item_id = {table}.id AND data.user_id = "
        );
        push_bind(sql, values, user_id, "");
        sql.push_str(" AND ");
        sql.push_str(predicate);
        sql.push_str(") = ");
        push_bind(sql, values, expected, "");
        sql.push(')');
    }
    if let Some(is_played) = query.is_played {
        sql.push_str(" AND");
        append_is_played_filter(sql, values, user_id, table, is_played);
    }
}

fn append_by_name_item_type_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    item_type: &str,
    table: &str,
) {
    let item_types = expand_item_type_aliases(&[item_type.to_owned()]);
    append_string_list_filter(
        sql,
        values,
        &format!("{table}.item_type"),
        &item_types,
        false,
    );
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

#[cfg(test)]
mod tests {
    use sea_orm::Value as SeaValue;

    use super::{ItemValueQuery, append_count_scope_filters, append_item_filters};

    #[test]
    fn item_value_filters_keep_discovery_only_exclusions_out_of_count_scope() {
        let query = ItemValueQuery {
            include_item_types: vec!["movie".to_owned()],
            exclude_item_types: vec!["EPISODE".to_owned()],
            discovery_exclude_item_types: vec!["audio".to_owned()],
            ..Default::default()
        };
        let mut matching_sql = String::new();
        let mut matching_values = Vec::new();

        append_item_filters(&mut matching_sql, &mut matching_values, &query);

        assert!(matching_sql.contains("item.item_type IN ($1, $2)"));
        assert!(matching_sql.contains("item.item_type NOT IN ($3, $4)"));
        assert!(matching_sql.contains("item.item_type NOT IN ($5, $6)"));
        assert_eq!(
            string_values(&matching_values),
            [
                "Movie",
                "MediaBrowser.Controller.Entities.Movies.Movie",
                "Episode",
                "MediaBrowser.Controller.Entities.TV.Episode",
                "Audio",
                "MediaBrowser.Controller.Entities.Audio.Audio",
            ]
        );

        let mut count_sql = String::new();
        let mut count_values = Vec::new();
        append_count_scope_filters(&mut count_sql, &mut count_values, &query, "counted");

        assert!(count_sql.contains("counted.item_type NOT IN ($1, $2)"));
        assert_eq!(
            string_values(&count_values),
            ["Episode", "MediaBrowser.Controller.Entities.TV.Episode"]
        );
        assert!(!count_sql.contains("Audio"));
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
