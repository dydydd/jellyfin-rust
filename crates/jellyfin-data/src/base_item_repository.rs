use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use chrono::{DateTime, Utc};
use jellyfin_extensions::StringExtensions;
use sea_orm::{
    AccessMode,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr,
    DeleteResult, EntityTrait, FromQueryResult, IsolationLevel, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, SqlErr, Statement, TransactionTrait, Value as SeaValue,
    sea_query::{Alias, Expr, Order, Query, extension::postgres::PgExpr},
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{ancestor_id, base_item, item_value, linked_child, user_data};

const HIERARCHY_ADVISORY_LOCK_KEY: i64 = 0x4241_5345_4954_454d;
pub const USER_ROOT_FOLDER_ID: Uuid = Uuid::from_u128(2);

/// Values accepted when creating a persisted Jellyfin base item.
#[derive(Debug, Clone, PartialEq)]
pub struct NewBaseItem {
    pub id: Uuid,
    pub item_type: String,
    pub data: Option<Value>,
    pub path: Option<String>,
    pub parent_id: Option<Uuid>,
    pub name: Option<String>,
    pub sort_name: Option<String>,
    pub media_type: Option<String>,
    pub overview: Option<String>,
    pub official_rating: Option<String>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
    pub premiere_date: Option<DateTime<Utc>>,
    pub runtime_ticks: Option<i64>,
    pub is_folder: bool,
    pub is_virtual_item: bool,
    pub presentation_unique_key: Option<String>,
    pub primary_version_id: Option<Uuid>,
    pub series_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    pub series_presentation_unique_key: Option<String>,
}

impl NewBaseItem {
    #[must_use]
    pub fn new(id: Uuid, item_type: impl Into<String>) -> Self {
        Self {
            id,
            item_type: item_type.into(),
            data: None,
            path: None,
            parent_id: None,
            name: None,
            sort_name: None,
            media_type: None,
            overview: None,
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BaseItemHierarchyEntry {
    pub item: base_item::Model,
    pub depth: i32,
}

/// Stable database ordering for base-item queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BaseItemOrder {
    #[default]
    SortName,
    SortNameDescending,
    DateCreatedAscending,
    DateCreatedDescending,
    DatePlayedAscending,
    DatePlayedDescending,
    PremiereDateAscending,
    PremiereDateDescending,
    Random,
    PlayCountAscending,
    PlayCountDescending,
    CommunityRatingAscending,
    CommunityRatingDescending,
    CriticRatingAscending,
    CriticRatingDescending,
    RuntimeTicksAscending,
    RuntimeTicksDescending,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BaseItemQuery {
    pub ids: Vec<Uuid>,
    pub exclude_ids: Vec<Uuid>,
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    pub tags: Vec<String>,
    pub person: Option<String>,
    pub min_community_rating: Option<f64>,
    pub is_favorite: Option<bool>,
    pub is_folder: Option<bool>,
    pub is_liked: Option<bool>,
    pub is_favorite_or_liked: Option<bool>,
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
    pub is_virtual_item: Option<bool>,
    pub group_versions_by_presentation_key: bool,
    pub user_id: Option<Uuid>,
    pub is_resumable: Option<bool>,
    pub is_played: Option<bool>,
    pub min_premiere_date: Option<DateTime<Utc>>,
    pub order: BaseItemOrder,
    pub start_index: u64,
    pub limit: Option<u64>,
    pub enable_total_record_count: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BaseItemPage {
    pub items: Vec<base_item::Model>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, FromQueryResult)]
pub struct BaseItemCounts {
    pub movie_count: i64,
    pub series_count: i64,
    pub episode_count: i64,
    pub artist_count: i64,
    pub program_count: i64,
    pub trailer_count: i64,
    pub song_count: i64,
    pub album_count: i64,
    pub music_video_count: i64,
    pub box_set_count: i64,
    pub book_count: i64,
    pub item_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionYearPage {
    pub years: Vec<i32>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProductionYearOrder {
    #[default]
    Ascending,
    Descending,
    Random,
}

#[derive(Debug, Error)]
pub enum BaseItemError {
    #[error("base item type cannot be empty")]
    InvalidItemType,
    #[error("base item was not found")]
    NotFound,
    #[error("base item parent was not found")]
    ParentNotFound,
    #[error("base item hierarchy cannot contain a cycle")]
    HierarchyCycle,
    #[error("protected base items cannot be deleted")]
    ProtectedItem,
    #[error("base item was changed by another writer")]
    StaleVersion,
    #[error("a user is required for playback-aware item queries")]
    UserRequired,
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed item metadata and hierarchy persistence.
#[derive(Clone)]
pub struct BaseItemRepository {
    database: DatabaseConnection,
}

impl BaseItemRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Returns the single persisted user-library root, creating it when the
    /// database has not been initialized yet.
    ///
    /// The hierarchy advisory lock makes concurrent server startups converge
    /// on one row. The reserved identifier also makes initialization
    /// idempotent across restarts.
    ///
    /// # Errors
    ///
    /// Returns a database error when the root cannot be loaded or created.
    pub async fn ensure_user_root(&self) -> Result<base_item::Model, BaseItemError> {
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;

        if let Some(root) = base_item::Entity::find()
            .filter(base_item::Column::ItemType.eq("UserRootFolder"))
            .filter(base_item::Column::ParentId.is_null())
            .order_by_asc(base_item::Column::DateCreated)
            .order_by_asc(base_item::Column::Id)
            .one(&transaction)
            .await?
        {
            transaction.commit().await?;
            return Ok(root);
        }

        let root = base_item::ActiveModel {
            id: Set(USER_ROOT_FOLDER_ID),
            item_type: Set("UserRootFolder".to_owned()),
            name: Set(Some("Root".to_owned())),
            sort_name: Set(Some("Root".to_owned())),
            is_folder: Set(true),
            ..Default::default()
        };
        let root = base_item::Entity::insert(root)
            .exec_with_returning(&transaction)
            .await
            .map_err(map_database_error)?;
        transaction.commit().await?;
        Ok(root)
    }

    /// Inserts an item and atomically maintains its closure-table rows.
    ///
    /// # Errors
    ///
    /// Returns a validation, hierarchy, or database error.
    pub async fn create(&self, item: NewBaseItem) -> Result<base_item::Model, BaseItemError> {
        validate_item_type(&item.item_type)?;
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        validate_parent(&transaction, item.id, item.parent_id).await?;

        let model = base_item::ActiveModel {
            id: Set(item.id),
            item_type: Set(item.item_type),
            data: Set(item.data),
            path: Set(item.path),
            parent_id: Set(item.parent_id),
            name: Set(item.name),
            sort_name: Set(item.sort_name),
            media_type: Set(item.media_type),
            overview: Set(item.overview),
            official_rating: Set(item.official_rating),
            index_number: Set(item.index_number),
            parent_index_number: Set(item.parent_index_number),
            production_year: Set(item.production_year),
            premiere_date: Set(item.premiere_date),
            runtime_ticks: Set(item.runtime_ticks),
            is_folder: Set(item.is_folder),
            is_virtual_item: Set(item.is_virtual_item),
            presentation_unique_key: Set(item.presentation_unique_key),
            primary_version_id: Set(item.primary_version_id),
            series_id: Set(item.series_id),
            season_id: Set(item.season_id),
            series_presentation_unique_key: Set(item.series_presentation_unique_key),
            ..Default::default()
        };
        let inserted = base_item::Entity::insert(model)
            .exec_with_returning(&transaction)
            .await
            .map_err(map_database_error)?;
        transaction.commit().await?;
        Ok(inserted)
    }

    /// Batch-inserts items in a single transaction.
    ///
    /// # Errors
    ///
    /// Returns a validation, hierarchy, or database error. On failure
    /// the entire batch is rolled back.
    pub async fn create_many(
        &self,
        items: Vec<NewBaseItem>,
    ) -> Result<Vec<base_item::Model>, BaseItemError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        for item in &items {
            validate_item_type(&item.item_type)?;
        }
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        for item in &items {
            validate_parent(&transaction, item.id, item.parent_id).await?;
        }
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let model = base_item::ActiveModel {
                id: Set(item.id),
                item_type: Set(item.item_type),
                data: Set(item.data),
                path: Set(item.path),
                parent_id: Set(item.parent_id),
                name: Set(item.name),
                sort_name: Set(item.sort_name),
                media_type: Set(item.media_type),
                overview: Set(item.overview),
                official_rating: Set(item.official_rating),
                index_number: Set(item.index_number),
                parent_index_number: Set(item.parent_index_number),
                production_year: Set(item.production_year),
                premiere_date: Set(item.premiere_date),
                runtime_ticks: Set(item.runtime_ticks),
                is_folder: Set(item.is_folder),
                is_virtual_item: Set(item.is_virtual_item),
                presentation_unique_key: Set(item.presentation_unique_key),
                primary_version_id: Set(item.primary_version_id),
                series_id: Set(item.series_id),
                season_id: Set(item.season_id),
                series_presentation_unique_key: Set(item.series_presentation_unique_key),
                ..Default::default()
            };
            let inserted = base_item::Entity::insert(model)
                .exec_with_returning(&transaction)
                .await
                .map_err(map_database_error)?;
            result.push(inserted);
        }
        transaction.commit().await?;
        Ok(result)
    }

    /// Loads an item by its stable identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get(&self, id: Uuid) -> Result<Option<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find_by_id(id)
            .one(&self.database)
            .await?)
    }

    /// Loads a set of items in one `PostgreSQL` query.
    ///
    /// The returned order is database-defined; callers that carry a separate
    /// presentation order should join the models by identifier.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get_many(&self, ids: &[Uuid]) -> Result<Vec<base_item::Model>, BaseItemError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Id.is_in(ids.iter().copied()))
            .all(&self.database)
            .await?)
    }

    /// Resolves a persisted item-by-name entity by its raw display name.
    ///
    /// The exact equality predicate intentionally preserves `PostgreSQL` text
    /// collation semantics instead of applying the fuzzy search normalization.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn get_by_type_and_name(
        &self,
        item_type: &str,
        name: &str,
    ) -> Result<Option<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::ItemType.eq(item_type))
            .filter(base_item::Column::Name.eq(name))
            .order_by_asc(base_item::Column::IsVirtualItem)
            .order_by_asc(base_item::Column::Id)
            .one(&self.database)
            .await?)
    }

    /// Resolves `version_item_id` when it is a video alternate visible from
    /// `source_item_id`.
    ///
    /// Jellyfin playback reports can carry the displayed item as `ItemId` and
    /// the actually played alternate version as `MediaSourceId`. `PostgreSQL`
    /// resolves the source group and target video in one indexed read; missing,
    /// non-video, or unrelated targets return `None` so callers can fall back to
    /// the displayed item.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn alternate_video_version(
        &self,
        source_item_id: Uuid,
        version_item_id: Uuid,
    ) -> Result<Option<base_item::Model>, BaseItemError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "WITH requested AS MATERIALIZED (\
                     SELECT COALESCE(primary_version_id, id) AS group_id \
                     FROM jellyfin.base_items \
                     WHERE id = $1 \
                       AND item_type IN ('Video', 'Movie', 'Episode', 'MusicVideo', 'Trailer')\
                 ), target_version AS (\
                     SELECT item.* \
                     FROM jellyfin.base_items AS item \
                     INNER JOIN requested \
                       ON COALESCE(item.primary_version_id, item.id) = requested.group_id \
                     WHERE item.id = $2 \
                       AND item.item_type IN ('Video', 'Movie', 'Episode', 'MusicVideo', 'Trailer')\
                 ) \
                 SELECT {BASE_ITEM_COLUMNS} FROM target_version"
            ),
            [source_item_id.into(), version_item_id.into()],
        );
        Ok(base_item::Model::find_by_statement(statement)
            .one(&self.database)
            .await?)
    }

    /// Reports whether an item identifier is present.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn exists(&self, id: Uuid) -> Result<bool, BaseItemError> {
        Ok(self.get(id).await?.is_some())
    }

    /// Loads items whose stored path exactly matches one of the supplied paths.
    ///
    /// Results use stable Jellyfin-style sort-name ordering and omit missing
    /// paths. Callers that need to preserve the source list can join by `path`
    /// afterwards.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn by_paths(&self, paths: &[String]) -> Result<Vec<base_item::Model>, BaseItemError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Path.is_in(paths.iter().cloned()))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .all(&self.database)
            .await?)
    }

    /// Detaches every member of the version group containing `item_id`.
    ///
    /// `PostgreSQL` resolves the primary identifier and clears the complete group
    /// in one data-modifying CTE. The statement's row locks serialize competing
    /// clears, while the surrounding transaction makes the group transition
    /// atomic. Rows and their media metadata are preserved.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when `item_id` is absent, or a database error when the
    /// transaction cannot be completed.
    pub async fn clear_alternate_sources(&self, item_id: Uuid) -> Result<(), BaseItemError> {
        let transaction = self.database.begin().await?;
        let result = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "WITH requested AS MATERIALIZED (\
                     SELECT COALESCE(primary_version_id, id) AS group_id \
                     FROM jellyfin.base_items \
                     WHERE id = $1\
                 ), cleared AS (\
                     UPDATE jellyfin.base_items AS item \
                     SET primary_version_id = NULL \
                     FROM requested \
                     WHERE item.id = requested.group_id \
                        OR item.primary_version_id = requested.group_id \
                     RETURNING item.id\
                 ) \
                 SELECT EXISTS (SELECT 1 FROM requested) AS found, \
                        COUNT(*) AS cleared_count \
                 FROM cleared",
                [item_id.into()],
            ))
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound("alternate-source clear returned no row".to_owned())
            })?;
        if !result.try_get::<bool>("", "found")? {
            return Err(BaseItemError::NotFound);
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Merges the supplied video identifiers and their existing version groups.
    ///
    /// `PostgreSQL` expands every requested row to its current version group,
    /// chooses a stable primary identifier, and rewrites all members in one
    /// data-modifying CTE. This preserves rows and media metadata while making
    /// concurrent merges serialize on the updated rows.
    ///
    /// # Errors
    ///
    /// Returns `InvalidItemType` when fewer than two existing rows are supplied,
    /// or a database error when the transaction cannot be completed.
    pub async fn merge_alternate_versions(&self, item_ids: &[Uuid]) -> Result<Uuid, BaseItemError> {
        if item_ids.len() < 2 {
            return Err(BaseItemError::InvalidItemType);
        }

        let mut values = Vec::with_capacity(item_ids.len());
        let mut sql = String::from("WITH requested(id) AS (VALUES ");
        for (index, item_id) in item_ids.iter().copied().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            values.push(item_id.into());
            let _ = write!(sql, "(${}::uuid)", values.len());
        }
        sql.push_str(
            "), requested_distinct AS MATERIALIZED (\
                 SELECT DISTINCT id FROM requested\
             ), requested_items AS MATERIALIZED (\
                 SELECT item.id, COALESCE(item.primary_version_id, item.id) AS group_id \
                 FROM jellyfin.base_items AS item \
                 INNER JOIN requested_distinct AS requested \
                   ON requested.id = item.id\
             ), merge_groups AS MATERIALIZED (\
                 SELECT DISTINCT group_id FROM requested_items\
             ), merge_members AS MATERIALIZED (\
                 SELECT DISTINCT item.id \
                 FROM jellyfin.base_items AS item \
                 INNER JOIN merge_groups \
                   ON item.id = merge_groups.group_id \
                   OR item.primary_version_id = merge_groups.group_id\
             ), primary_version AS MATERIALIZED (\
                 SELECT id FROM merge_members ORDER BY id LIMIT 1\
             ), updated AS (\
                 UPDATE jellyfin.base_items AS item \
                 SET primary_version_id = CASE \
                         WHEN item.id = (SELECT id FROM primary_version) THEN NULL \
                         ELSE (SELECT id FROM primary_version) \
                     END \
                 FROM merge_members \
                 WHERE item.id = merge_members.id \
                 RETURNING item.id\
             ) \
             SELECT (SELECT COUNT(*) FROM requested_items) AS requested_count, \
                    (SELECT id FROM primary_version) AS primary_id",
        );

        let transaction = self.database.begin().await?;
        let result = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("version merge returned no row".to_owned()))?;
        let requested_count = result.try_get::<i64>("", "requested_count")?;
        if requested_count < 2 {
            return Err(BaseItemError::InvalidItemType);
        }
        let primary_id = result.try_get::<Uuid>("", "primary_id")?;
        transaction.commit().await?;
        Ok(primary_id)
    }

    /// Uses the `PostgreSQL` partial hash index to test an exact item path.
    ///
    /// # Errors
    ///
    /// Returns a database error when the lookup fails.
    pub async fn exists_by_path(&self, path: &str) -> Result<bool, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::Path.eq(path))
            .one(&self.database)
            .await?
            .is_some())
    }

    /// Loads a persisted Jellyfin `Year` item by its display name.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn year_item(&self, year: i32) -> Result<Option<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::ItemType.eq("Year"))
            .filter(base_item::Column::Name.eq(year.to_string()))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .one(&self.database)
            .await?)
    }

    /// Returns true when at least one persisted item advertises the production year.
    ///
    /// This intentionally uses a selective `LIMIT 1` lookup so PostgreSQL can
    /// satisfy year endpoints from the partial production-year index.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn has_production_year(&self, year: i32) -> Result<bool, BaseItemError> {
        Ok(base_item::Entity::find()
            .select_only()
            .column(base_item::Column::Id)
            .filter(base_item::Column::ProductionYear.eq(year))
            .filter(base_item::Column::ItemType.ne("PLACEHOLDER"))
            .limit(1)
            .into_tuple::<Uuid>()
            .one(&self.database)
            .await?
            .is_some())
    }

    /// Queries distinct positive production years through PostgreSQL.
    ///
    /// # Errors
    ///
    /// Returns a database error when the filtered distinct-year query fails.
    pub async fn production_years(
        &self,
        query: &BaseItemQuery,
        order: ProductionYearOrder,
    ) -> Result<ProductionYearPage, BaseItemError> {
        let (cte, values) = production_years_cte(query);
        let transaction = self
            .database
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await?;
        let count = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("{cte} SELECT COUNT(*) AS total_record_count FROM years"),
                values.clone(),
            ))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("year count returned no row".to_owned()))?
            .try_get::<i64>("", "total_record_count")?;

        let mut year_values = values;
        let order = match order {
            ProductionYearOrder::Ascending => "production_year ASC",
            ProductionYearOrder::Descending => "production_year DESC",
            ProductionYearOrder::Random => "random(), production_year ASC",
        };
        let mut year_sql = format!("{cte} SELECT production_year FROM years ORDER BY {order}");
        push_bind(
            &mut year_sql,
            &mut year_values,
            i64::try_from(query.start_index).unwrap_or(i64::MAX),
            " OFFSET ",
        );
        if let Some(limit) = query.limit {
            push_bind(
                &mut year_sql,
                &mut year_values,
                i64::try_from(limit).unwrap_or(i64::MAX),
                " LIMIT ",
            );
        }
        let years = transaction
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                year_sql,
                year_values,
            ))
            .await?
            .into_iter()
            .map(|row| row.try_get::<i32>("", "production_year"))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await?;
        Ok(ProductionYearPage {
            years,
            total_record_count: u64::try_from(count).unwrap_or_default(),
            start_index: query.start_index,
        })
    }

    /// Queries distinct non-empty official ratings through PostgreSQL.
    ///
    /// The `base_items_official_rating_idx` partial index keeps the legacy
    /// filter endpoint selective even on large libraries where most rows do
    /// not carry a parental rating.
    ///
    /// # Errors
    ///
    /// Returns a database error when the filtered distinct-rating query fails.
    pub async fn official_ratings(
        &self,
        query: &BaseItemQuery,
    ) -> Result<Vec<String>, BaseItemError> {
        let (cte, values) = official_ratings_cte(query);
        Ok(self
            .database
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("{cte} SELECT official_rating FROM ratings ORDER BY official_rating ASC"),
                values,
            ))
            .await?
            .into_iter()
            .map(|row| row.try_get::<String>("", "official_rating"))
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Queries persisted library items with stable sorting and database-side
    /// count, offset, and limit.
    ///
    /// # Errors
    ///
    /// Returns a database error when hierarchy or item queries fail.
    pub async fn query(&self, query: &BaseItemQuery) -> Result<BaseItemPage, BaseItemError> {
        if let Some(is_resumable) = query.is_resumable {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            return if is_resumable {
                self.query_resumable(user_id, query).await
            } else {
                self.query_not_resumable(user_id, query).await
            };
        }
        if matches!(
            query.order,
            BaseItemOrder::DatePlayedAscending | BaseItemOrder::DatePlayedDescending
        ) {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            return self.query_by_date_played(user_id, query).await;
        }
        if matches!(
            query.order,
            BaseItemOrder::PlayCountAscending
                | BaseItemOrder::PlayCountDescending
                | BaseItemOrder::CommunityRatingAscending
                | BaseItemOrder::CommunityRatingDescending
                | BaseItemOrder::CriticRatingAscending
                | BaseItemOrder::CriticRatingDescending
                | BaseItemOrder::RuntimeTicksAscending
                | BaseItemOrder::RuntimeTicksDescending
        ) {
            return self.query_by_extended_sort(query).await;
        }
        if query.group_versions_by_presentation_key {
            return self.query_grouped_versions(query).await;
        }
        let mut select =
            base_item::Entity::find().filter(base_item::Column::ItemType.ne("PLACEHOLDER"));
        if !query.ids.is_empty() {
            select = select.filter(base_item::Column::Id.is_in(query.ids.iter().copied()));
        }
        if !query.exclude_ids.is_empty() {
            select =
                select.filter(base_item::Column::Id.is_not_in(query.exclude_ids.iter().copied()));
        }
        if let Some(parent_id) = query.parent_id {
            if query.recursive {
                let descendants = Query::select()
                    .column(ancestor_id::Column::ItemId)
                    .from((Alias::new("jellyfin"), ancestor_id::Entity))
                    .and_where(ancestor_id::Column::ParentItemId.eq(parent_id))
                    .to_owned();
                select = select.filter(base_item::Column::Id.in_subquery(descendants));
            } else {
                select = select.filter(base_item::Column::ParentId.eq(parent_id));
            }
        }
        if let Some(search_term) = query
            .search_term
            .as_deref()
            .map(str::trim)
            .filter(|term| !term.is_empty())
        {
            let clean_search_term = search_term.clean_value();
            select = select.filter(
                Expr::col(base_item::Column::CleanName)
                    .ilike(postgres_contains_pattern(&clean_search_term)),
            );
        }
        if !query.include_item_types.is_empty() {
            select = select.filter(
                base_item::Column::ItemType.is_in(query.include_item_types.iter().cloned()),
            );
        }
        if !query.exclude_item_types.is_empty() {
            select = select.filter(
                base_item::Column::ItemType.is_not_in(query.exclude_item_types.iter().cloned()),
            );
        }
        if !query.media_types.is_empty() {
            select = select
                .filter(base_item::Column::MediaType.is_in(query.media_types.iter().cloned()));
        }
        if let Some(is_movie) = query.is_movie {
            select = select.filter(media_class_condition(
                is_movie,
                "IsMovie",
                &["Movie", "Trailer"],
            ));
        }
        if let Some(is_series) = query.is_series {
            select = select.filter(media_class_condition(is_series, "IsSeries", &["Series"]));
        }
        if let Some(is_sports) = query.is_sports {
            select = select.filter(tag_class_condition(is_sports, "sports"));
        }
        if let Some(is_news) = query.is_news {
            select = select.filter(tag_class_condition(is_news, "news"));
        }
        if let Some(is_kids) = query.is_kids {
            select = select.filter(tag_class_condition(is_kids, "kids"));
        }
        if let Some(is_virtual_item) = query.is_virtual_item {
            select = select.filter(base_item::Column::IsVirtualItem.eq(is_virtual_item));
        }
        if let Some(is_played) = query.is_played {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            let played_items = Query::select()
                .column(user_data::Column::ItemId)
                .from((Alias::new("jellyfin"), user_data::Entity))
                .and_where(user_data::Column::UserId.eq(user_id))
                .and_where(user_data::Column::Played.eq(true))
                .to_owned();
            select = if is_played {
                select.filter(base_item::Column::Id.in_subquery(played_items))
            } else {
                select.filter(base_item::Column::Id.not_in_subquery(played_items))
            };
        }
        if let Some(is_favorite) = query.is_favorite {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            let favorite_items = Query::select()
                .column(user_data::Column::ItemId)
                .from((Alias::new("jellyfin"), user_data::Entity))
                .and_where(user_data::Column::UserId.eq(user_id))
                .and_where(user_data::Column::IsFavorite.eq(true))
                .to_owned();
            select = if is_favorite {
                select.filter(base_item::Column::Id.in_subquery(favorite_items))
            } else {
                select.filter(base_item::Column::Id.not_in_subquery(favorite_items))
            };
        }
        if let Some(is_folder) = query.is_folder {
            select = select.filter(base_item::Column::IsFolder.eq(is_folder));
        }
        if let Some(is_liked) = query.is_liked {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            let liked_items = Query::select()
                .column(user_data::Column::ItemId)
                .from((Alias::new("jellyfin"), user_data::Entity))
                .and_where(user_data::Column::UserId.eq(user_id))
                .and_where(user_data::Column::Likes.eq(is_liked))
                .to_owned();
            select = if is_liked {
                select.filter(base_item::Column::Id.in_subquery(liked_items))
            } else {
                select.filter(base_item::Column::Id.not_in_subquery(liked_items))
            };
        }
        if let Some(is_favorite_or_liked) = query.is_favorite_or_liked {
            let user_id = query.user_id.ok_or(BaseItemError::UserRequired)?;
            let favorite_or_liked_items = Query::select()
                .column(user_data::Column::ItemId)
                .from((Alias::new("jellyfin"), user_data::Entity))
                .and_where(user_data::Column::UserId.eq(user_id))
                .and_where(
                    user_data::Column::IsFavorite
                        .eq(true)
                        .or(user_data::Column::Likes.eq(true)),
                )
                .to_owned();
            select = if is_favorite_or_liked {
                select.filter(base_item::Column::Id.in_subquery(favorite_or_liked_items))
            } else {
                select.filter(base_item::Column::Id.not_in_subquery(favorite_or_liked_items))
            };
        }
        if !query.genres.is_empty() {
            select = select.filter(item_value_exists_expression(
                "base_items",
                "id",
                item_value::ItemValueType::Genre,
                &query.genres,
            ));
        }
        if !query.tags.is_empty() {
            select = select.filter(item_value_exists_expression(
                "base_items",
                "id",
                item_value::ItemValueType::Tags,
                &query.tags,
            ));
        }
        if !query.years.is_empty() {
            select = select.filter(base_item::Column::ProductionYear.is_in(query.years.iter().copied()));
        }
        if let Some(person) = query.person.as_deref().filter(|value| !value.trim().is_empty()) {
            select = select.filter(person_exists_expression(person));
        }
        if let Some(min_community_rating) = query.min_community_rating {
            select = select.filter(community_rating_expression(min_community_rating));
        }
        if let Some(min_premiere_date) = query.min_premiere_date {
            select = select.filter(base_item::Column::PremiereDate.gte(min_premiere_date));
        }
        let total_record_count = if total_count_enabled(query) {
            Some(select.clone().count(&self.database).await?)
        } else {
            None
        };
        let mut select = match query.order {
            BaseItemOrder::SortName => select.order_by_asc(base_item::Column::SortName),
            BaseItemOrder::SortNameDescending => select.order_by_desc(base_item::Column::SortName),
            BaseItemOrder::DateCreatedAscending => {
                select.order_by_asc(base_item::Column::DateCreated)
            }
            BaseItemOrder::DateCreatedDescending => {
                select.order_by_desc(base_item::Column::DateCreated)
            }
            BaseItemOrder::Random => select.order_by(Expr::cust("random()"), Order::Asc),
            BaseItemOrder::DatePlayedAscending | BaseItemOrder::DatePlayedDescending => {
                unreachable!("date-played queries are handled by query_by_date_played")
            }
            BaseItemOrder::PremiereDateAscending => {
                select.order_by_asc(base_item::Column::PremiereDate)
            }
            BaseItemOrder::PremiereDateDescending => {
                select.order_by_desc(base_item::Column::PremiereDate)
            }
            _ => select.order_by_asc(base_item::Column::SortName),
        }
        .order_by_asc(base_item::Column::Id)
        .offset(query.start_index);
        if let Some(limit) = query.limit {
            select = select.limit(limit);
        }
        let items = select.all(&self.database).await?;
        Ok(BaseItemPage {
            total_record_count: page_total_record_count(total_record_count, items.len()),
            items,
            start_index: query.start_index,
        })
    }

    /// Counts non-virtual library items by Jellyfin's public item-count buckets.
    ///
    /// `PostgreSQL` computes all buckets in one aggregate scan using `FILTER`
    /// clauses. When `is_favorite` is provided for a user, the filter treats
    /// missing user-data rows as not favorite, matching Jellyfin's public query
    /// semantics.
    ///
    /// # Errors
    ///
    /// Returns a database error when the aggregate query fails.
    pub async fn item_counts(
        &self,
        user_id: Option<Uuid>,
        is_favorite: Option<bool>,
    ) -> Result<BaseItemCounts, BaseItemError> {
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
            SELECT
                COUNT(*) FILTER (WHERE item.item_type = 'Movie')::bigint AS movie_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Series')::bigint AS series_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Episode')::bigint AS episode_count,
                COUNT(*) FILTER (WHERE item.item_type = 'MusicArtist')::bigint AS artist_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Program')::bigint AS program_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Trailer')::bigint AS trailer_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Audio')::bigint AS song_count,
                COUNT(*) FILTER (WHERE item.item_type = 'MusicAlbum')::bigint AS album_count,
                COUNT(*) FILTER (WHERE item.item_type = 'MusicVideo')::bigint AS music_video_count,
                COUNT(*) FILTER (WHERE item.item_type = 'BoxSet')::bigint AS box_set_count,
                COUNT(*) FILTER (WHERE item.item_type = 'Book')::bigint AS book_count,
                COUNT(*)::bigint AS item_count
            FROM jellyfin.base_items AS item
            WHERE item.item_type <> 'PLACEHOLDER'
              AND item.is_virtual_item = false
              AND (
                  $1::boolean IS NULL
                  OR (
                      $2::uuid IS NOT NULL
                      AND (
                          ($1 = true AND EXISTS (
                              SELECT 1
                              FROM jellyfin.user_data AS data
                              WHERE data.item_id = item.id
                                AND data.user_id = $2
                                AND data.is_favorite
                          ))
                          OR ($1 = false AND NOT EXISTS (
                              SELECT 1
                              FROM jellyfin.user_data AS data
                              WHERE data.item_id = item.id
                                AND data.user_id = $2
                                AND data.is_favorite
                          ))
                      )
                  )
                  OR ($2::uuid IS NULL AND $1 = false)
              )
            ",
            vec![is_favorite.into(), user_id.into()],
        );
        base_item::Model::find_by_statement(statement)
            .into_model::<BaseItemCounts>()
            .one(&self.database)
            .await?
            .ok_or_else(|| {
                BaseItemError::Database(DbErr::RecordNotFound(
                    "item counts returned no row".to_owned(),
                ))
            })
    }

    async fn query_grouped_versions(
        &self,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let (cte, values) = grouped_versions_cte(query);
        let transaction = self
            .database
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await?;
        let count = if total_count_enabled(query) {
            Some(
                transaction
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        format!("{cte} SELECT COUNT(*) AS total_record_count FROM version_groups"),
                        values.clone(),
                    ))
                    .await?
                    .ok_or_else(|| {
                        DbErr::RecordNotFound("grouped item count returned no row".to_owned())
                    })?
                    .try_get::<i64>("", "total_record_count")?,
            )
        } else {
            None
        };

        let mut item_values = values;
        let mut item_sql = format!(
            "{cte} SELECT {BASE_ITEM_COLUMNS} FROM version_groups \
             ORDER BY sort_name, id"
        );
        push_bind(
            &mut item_sql,
            &mut item_values,
            i64::try_from(query.start_index).unwrap_or(i64::MAX),
            " OFFSET ",
        );
        if let Some(limit) = query.limit {
            push_bind(
                &mut item_sql,
                &mut item_values,
                i64::try_from(limit).unwrap_or(i64::MAX),
                " LIMIT ",
            );
        }
        let items = base_item::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            item_sql,
            item_values,
        ))
        .all(&transaction)
        .await?;
        transaction.commit().await?;
        Ok(BaseItemPage {
            total_record_count: count
                .map(|count| u64::try_from(count).unwrap_or_default())
                .unwrap_or_else(|| u64::try_from(items.len()).unwrap_or(u64::MAX)),
            items,
            start_index: query.start_index,
        })
    }

    /// Queries resumable items with PostgreSQL-side legacy-key deduplication,
    /// item filtering, recency ordering, counting, and pagination.
    ///
    /// # Errors
    ///
    /// Returns a database error when the count or item query fails.
    pub async fn query_resumable(
        &self,
        user_id: Uuid,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let (cte, values) = resumable_filtered_cte(user_id, query);
        self.query_raw_page(
            cte,
            values,
            "filtered",
            "resume_last_played_date DESC NULLS LAST, id",
            "resume",
            query,
        )
        .await
    }

    async fn query_not_resumable(
        &self,
        user_id: Uuid,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let (cte, values) = not_resumable_filtered_cte(user_id, query);
        self.query_raw_page(
            cte,
            values,
            "filtered",
            "sort_name, id",
            "not-resumable",
            query,
        )
        .await
    }

    async fn query_by_date_played(
        &self,
        user_id: Uuid,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let (cte, values) = date_played_filtered_cte(user_id, query);
        let order = match query.order {
            BaseItemOrder::DatePlayedAscending => "date_played ASC NULLS FIRST, id",
            BaseItemOrder::DatePlayedDescending => "date_played DESC NULLS LAST, id",
            BaseItemOrder::DateCreatedAscending => "date_created ASC, id",
            BaseItemOrder::DateCreatedDescending => "date_created DESC, id",
            BaseItemOrder::PremiereDateAscending => "premiere_date ASC NULLS LAST, sort_name, id",
            BaseItemOrder::PremiereDateDescending => {
                "premiere_date DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::SortName => "sort_name, id",
            BaseItemOrder::SortNameDescending => "sort_name DESC, id",
            BaseItemOrder::Random => "random(), id",
            _ => "sort_name, id",
        };
        self.query_raw_page(cte, values, "dated", order, "DatePlayed", query)
            .await
    }

    async fn query_by_extended_sort(
        &self,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let (cte, values) = extended_sort_cte(query);
        let order = match query.order {
            BaseItemOrder::PlayCountAscending => "play_count ASC NULLS LAST, sort_name, id",
            BaseItemOrder::PlayCountDescending => "play_count DESC NULLS LAST, sort_name, id",
            BaseItemOrder::CommunityRatingAscending => {
                "community_rating ASC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::CommunityRatingDescending => {
                "community_rating DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::CriticRatingAscending => "critic_rating ASC NULLS LAST, sort_name, id",
            BaseItemOrder::CriticRatingDescending => {
                "critic_rating DESC NULLS LAST, sort_name, id"
            }
            BaseItemOrder::RuntimeTicksAscending => "runtime_ticks ASC NULLS LAST, sort_name, id",
            BaseItemOrder::RuntimeTicksDescending => "runtime_ticks DESC NULLS LAST, sort_name, id",
            _ => unreachable!("extended-sort query only handles extended orders"),
        };
        self.query_raw_page(cte, values, "filtered", order, "ExtendedSort", query)
            .await
    }

    /// Queries the next unwatched episode for each eligible series.
    ///
    /// A series is eligible when it has at least one unwatched episode and
    /// either the user has already started watching it (`enable_rewatching`
    /// false) or rewatching is enabled. Each series contributes at most its
    /// earliest unwatched episode.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn next_up(
        &self,
        user_id: Uuid,
        parent_id: Option<Uuid>,
        enable_rewatching: bool,
        enable_resumable: bool,
        next_up_date_cutoff: Option<DateTime<Utc>>,
        start_index: u64,
        limit: Option<u64>,
        enable_total_record_count: bool,
    ) -> Result<BaseItemPage, BaseItemError> {
        let mut values = vec![user_id.into()];
        let mut sql = String::from(
            "WITH watched AS (\
                 SELECT DISTINCT data.item_id, data.played \
                 FROM jellyfin.user_data AS data \
                 WHERE data.user_id = $1 \
             ), eligible AS (\
                 SELECT episode.id AS episode_id, episode.series_id, \
                        episode.parent_index_number AS season_number, \
                        episode.index_number AS episode_number, \
                        episode.sort_name, \
                        EXISTS (SELECT 1 FROM watched WHERE watched.item_id = episode.id AND watched.played) AS is_watched, \
                        NOT EXISTS (SELECT 1 FROM watched WHERE watched.item_id = episode.id AND watched.played) AS is_unwatched \
                 FROM jellyfin.base_items AS episode \
                 WHERE episode.item_type = 'Episode' \
                   AND episode.is_virtual_item = false \
                   AND episode.series_id IS NOT NULL",
        );
        if let Some(parent_id) = parent_id {
            values.push(parent_id.into());
            let _ = write!(sql, " AND episode.id IN (\
                SELECT closure.item_id FROM jellyfin.ancestor_ids AS closure \
                WHERE closure.parent_item_id = ${}\
            )", values.len());
        }
        if !enable_resumable {
            sql.push_str(
                " AND NOT EXISTS (\
                    SELECT 1 FROM jellyfin.user_data AS resume \
                    WHERE resume.item_id = episode.id \
                      AND resume.user_id = $1 \
                      AND resume.playback_position_ticks > 0\
                )",
            );
        }
        if let Some(cutoff) = next_up_date_cutoff {
            values.push(cutoff.into());
            let _ = write!(sql, " AND episode.premiere_date >= ${}", values.len());
        }
        sql.push_str(
            "), ranked AS (\
                 SELECT eligible.*, \
                        ROW_NUMBER() OVER (\
                            PARTITION BY eligible.series_id \
                            ORDER BY eligible.season_number NULLS LAST, \
                                     eligible.episode_number NULLS LAST, \
                                     eligible.sort_name, eligible.episode_id\
                        ) AS episode_rank, \
                        SUM(CASE WHEN eligible.is_watched THEN 1 ELSE 0 END) \
                            OVER (PARTITION BY eligible.series_id) AS watched_count \
                 FROM eligible\
             ), next_episodes AS (\
                 SELECT ranked.* FROM ranked \
                 WHERE ranked.episode_rank = 1 \
                   AND ranked.is_unwatched \
                   AND (",
        );
        if enable_rewatching {
            sql.push_str("1 = 1");
        } else {
            sql.push_str("ranked.watched_count > 0");
        }
        sql.push_str(
            ")\
             ), selected AS (\
                 SELECT item.* FROM next_episodes AS next_episode \
                 JOIN jellyfin.base_items AS item ON item.id = next_episode.episode_id\
             )",
        );
        let total = if enable_total_record_count {
            Some(
                self.database
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        format!("{sql} SELECT COUNT(*) AS total_record_count FROM selected"),
                        values.clone(),
                    ))
                    .await?
                    .ok_or_else(|| {
                        DbErr::RecordNotFound("next-up count returned no row".to_owned())
                    })?
                    .try_get::<i64>("", "total_record_count")?,
            )
        } else {
            None
        };
        let mut page_values = values;
        let mut page_sql = format!(
            "{sql} SELECT {BASE_ITEM_COLUMNS} FROM selected \
             ORDER BY sort_name, id"
        );
        push_bind(
            &mut page_sql,
            &mut page_values,
            i64::try_from(start_index).unwrap_or(i64::MAX),
            " OFFSET ",
        );
        if let Some(limit) = limit {
            push_bind(
                &mut page_sql,
                &mut page_values,
                i64::try_from(limit).unwrap_or(i64::MAX),
                " LIMIT ",
            );
        }
        let items = base_item::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            page_sql,
            page_values,
        ))
        .all(&self.database)
        .await?;
        Ok(BaseItemPage {
            total_record_count: total.map_or_else(
                || u64::try_from(items.len()).unwrap_or(u64::MAX),
                |count| u64::try_from(count).unwrap_or_default(),
            ),
            items,
            start_index,
        })
    }

    async fn query_raw_page(
        &self,
        cte: String,
        values: Vec<SeaValue>,
        source: &str,
        order: &str,
        query_name: &str,
        query: &BaseItemQuery,
    ) -> Result<BaseItemPage, BaseItemError> {
        let transaction = self
            .database
            .begin_with_config(
                Some(IsolationLevel::RepeatableRead),
                Some(AccessMode::ReadOnly),
            )
            .await?;
        let count = if total_count_enabled(query) {
            Some(
                transaction
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        format!("{cte} SELECT COUNT(*) AS total_record_count FROM {source}"),
                        values.clone(),
                    ))
                    .await?
                    .ok_or_else(|| {
                        DbErr::RecordNotFound(format!("{query_name} count returned no row"))
                    })?
                    .try_get::<i64>("", "total_record_count")?,
            )
        } else {
            None
        };

        let mut item_values = values;
        let mut item_sql =
            format!("{cte} SELECT {BASE_ITEM_COLUMNS} FROM {source} ORDER BY {order}");
        push_bind(
            &mut item_sql,
            &mut item_values,
            i64::try_from(query.start_index).unwrap_or(i64::MAX),
            " OFFSET ",
        );
        if let Some(limit) = query.limit {
            push_bind(
                &mut item_sql,
                &mut item_values,
                i64::try_from(limit).unwrap_or(i64::MAX),
                " LIMIT ",
            );
        }
        let items = base_item::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            item_sql,
            item_values,
        ))
        .all(&transaction)
        .await?;
        transaction.commit().await?;
        Ok(BaseItemPage {
            total_record_count: count
                .map(|count| u64::try_from(count).unwrap_or_default())
                .unwrap_or_else(|| u64::try_from(items.len()).unwrap_or(u64::MAX)),
            items,
            start_index: query.start_index,
        })
    }

    /// Replaces mutable fields using `row_version` as an optimistic lock.
    ///
    /// # Errors
    ///
    /// Returns `StaleVersion` when another writer already updated the row.
    pub async fn update(&self, item: base_item::Model) -> Result<base_item::Model, BaseItemError> {
        validate_item_type(&item.item_type)?;
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        let current = base_item::Entity::find_by_id(item.id)
            .one(&transaction)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if current.row_version != item.row_version {
            return Err(BaseItemError::StaleVersion);
        }
        validate_parent(&transaction, item.id, item.parent_id).await?;

        let changes = base_item::ActiveModel {
            item_type: Set(item.item_type),
            data: Set(item.data),
            path: Set(item.path),
            parent_id: Set(item.parent_id),
            name: Set(item.name),
            sort_name: Set(item.sort_name),
            media_type: Set(item.media_type),
            overview: Set(item.overview),
            official_rating: Set(item.official_rating),
            index_number: Set(item.index_number),
            parent_index_number: Set(item.parent_index_number),
            production_year: Set(item.production_year),
            premiere_date: Set(item.premiere_date),
            runtime_ticks: Set(item.runtime_ticks),
            is_folder: Set(item.is_folder),
            is_virtual_item: Set(item.is_virtual_item),
            presentation_unique_key: Set(item.presentation_unique_key),
            primary_version_id: Set(item.primary_version_id),
            series_id: Set(item.series_id),
            season_id: Set(item.season_id),
            series_presentation_unique_key: Set(item.series_presentation_unique_key),
            ..Default::default()
        };
        let result = base_item::Entity::update_many()
            .set(changes)
            .filter(base_item::Column::Id.eq(item.id))
            .filter(base_item::Column::RowVersion.eq(item.row_version))
            .exec(&transaction)
            .await
            .map_err(map_database_error)?;
        if result.rows_affected == 0 {
            return Err(BaseItemError::StaleVersion);
        }
        let updated = base_item::Entity::find_by_id(item.id)
            .one(&transaction)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        transaction.commit().await?;
        Ok(updated)
    }

    /// Batch-updates items in a single transaction. Stale-version errors for
    /// individual items are silently skipped.
    ///
    /// # Errors
    ///
    /// Returns a validation or database error when the batch cannot be
    /// processed. On failure the entire batch is rolled back.
    pub async fn update_many(
        &self,
        items: Vec<base_item::Model>,
    ) -> Result<Vec<base_item::Model>, BaseItemError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        for item in &items {
            validate_item_type(&item.item_type)?;
        }
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let current = base_item::Entity::find_by_id(item.id)
                .one(&transaction)
                .await?;
            let Some(current) = current else {
                continue;
            };
            if current.row_version != item.row_version {
                continue;
            }
            match validate_parent(&transaction, item.id, item.parent_id).await {
                Err(BaseItemError::ParentNotFound | BaseItemError::HierarchyCycle) => continue,
                Err(error) => return Err(error),
                Ok(()) => {}
            }
            let changes = base_item::ActiveModel {
                item_type: Set(item.item_type),
                data: Set(item.data),
                path: Set(item.path),
                parent_id: Set(item.parent_id),
                name: Set(item.name),
                sort_name: Set(item.sort_name),
                media_type: Set(item.media_type),
                overview: Set(item.overview),
                official_rating: Set(item.official_rating),
                index_number: Set(item.index_number),
                parent_index_number: Set(item.parent_index_number),
                production_year: Set(item.production_year),
                premiere_date: Set(item.premiere_date),
                runtime_ticks: Set(item.runtime_ticks),
                is_folder: Set(item.is_folder),
                is_virtual_item: Set(item.is_virtual_item),
                presentation_unique_key: Set(item.presentation_unique_key),
                primary_version_id: Set(item.primary_version_id),
                series_id: Set(item.series_id),
                season_id: Set(item.season_id),
                series_presentation_unique_key: Set(item.series_presentation_unique_key),
                ..Default::default()
            };
            let update_result = base_item::Entity::update_many()
                .set(changes)
                .filter(base_item::Column::Id.eq(item.id))
                .filter(base_item::Column::RowVersion.eq(item.row_version))
                .exec(&transaction)
                .await
                .map_err(map_database_error)?;
            if update_result.rows_affected == 0 {
                continue;
            }
            if let Ok(updated) = base_item::Entity::find_by_id(item.id)
                .one(&transaction)
                .await
            {
                if let Some(updated) = updated {
                    result.push(updated);
                }
            }
        }
        transaction.commit().await?;
        Ok(result)
    }

    /// Moves an item while preserving optimistic-lock semantics.
    ///
    /// # Errors
    ///
    /// Returns hierarchy, stale-version, not-found, or database errors.
    pub async fn move_item(
        &self,
        id: Uuid,
        parent_id: Option<Uuid>,
        expected_row_version: i64,
    ) -> Result<base_item::Model, BaseItemError> {
        let mut item = self.get(id).await?.ok_or(BaseItemError::NotFound)?;
        if item.row_version != expected_row_version {
            return Err(BaseItemError::StaleVersion);
        }
        item.parent_id = parent_id;
        self.update(item).await
    }

    /// Deletes an item; the database cascades through the complete subtree.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn delete(&self, id: Uuid) -> Result<bool, BaseItemError> {
        match self.delete_many(&[id]).await {
            Ok(()) => Ok(true),
            Err(BaseItemError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Atomically deletes complete item subtrees and their detached user data.
    ///
    /// `PostgreSQL` foreign keys cascade through hierarchy and item mapping
    /// tables. `user_data` intentionally accepts detached item identifiers, so
    /// it is locked and cleaned explicitly in the same transaction.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` without deleting anything when any requested item is
    /// absent, `ProtectedItem` for the placeholder or user root, or a database
    /// error when deletion fails.
    pub async fn delete_many(&self, ids: &[Uuid]) -> Result<(), BaseItemError> {
        let ids = ids.iter().copied().collect::<HashSet<_>>();
        if ids.is_empty() {
            return Ok(());
        }
        if ids.contains(&Uuid::from_u128(1)) || ids.contains(&USER_ROOT_FOLDER_ID) {
            return Err(BaseItemError::ProtectedItem);
        }
        let transaction = self.database.begin().await?;
        acquire_hierarchy_lock(&transaction).await?;
        transaction
            .execute_unprepared("LOCK TABLE jellyfin.user_data IN SHARE ROW EXCLUSIVE MODE")
            .await?;
        let ids = ids.into_iter().collect::<Vec<_>>();
        let existing = base_item::Entity::find()
            .filter(base_item::Column::Id.is_in(ids.iter().copied()))
            .all(&transaction)
            .await?;
        if existing.len() != ids.len() {
            return Err(BaseItemError::NotFound);
        }

        let descendants = ancestor_id::Entity::find()
            .filter(ancestor_id::Column::ParentItemId.is_in(ids.iter().copied()))
            .all(&transaction)
            .await?;
        let affected_ids = ids
            .iter()
            .copied()
            .chain(descendants.into_iter().map(|row| row.item_id))
            .collect::<HashSet<_>>();
        linked_child::Entity::delete_many()
            .filter(
                linked_child::Column::ParentId
                    .is_in(affected_ids.iter().copied())
                    .or(linked_child::Column::ChildId.is_in(affected_ids.iter().copied())),
            )
            .exec(&transaction)
            .await?;
        user_data::Entity::delete_many()
            .filter(user_data::Column::ItemId.is_in(affected_ids.iter().copied()))
            .exec(&transaction)
            .await?;
        let DeleteResult { rows_affected } = base_item::Entity::delete_many()
            .filter(base_item::Column::Id.is_in(ids.iter().copied()))
            .exec(&transaction)
            .await?;
        if usize::try_from(rows_affected).unwrap_or(usize::MAX) != ids.len() {
            return Err(BaseItemError::NotFound);
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Loads the direct parent of an item.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the item itself is absent.
    pub async fn parent(&self, id: Uuid) -> Result<Option<base_item::Model>, BaseItemError> {
        let item = self.get(id).await?.ok_or(BaseItemError::NotFound)?;
        match item.parent_id {
            Some(parent_id) => self.get(parent_id).await,
            None => Ok(None),
        }
    }

    /// Loads direct children in stable sort-name and identifier order.
    ///
    /// # Errors
    ///
    /// Returns a database error when the query fails.
    pub async fn children(&self, id: Uuid) -> Result<Vec<base_item::Model>, BaseItemError> {
        Ok(base_item::Entity::find()
            .filter(base_item::Column::ParentId.eq(id))
            .order_by_asc(base_item::Column::SortName)
            .order_by_asc(base_item::Column::Id)
            .all(&self.database)
            .await?)
    }

    /// Loads all ancestors nearest-first with their closure-table depths.
    ///
    /// # Errors
    ///
    /// Returns a database error when either query fails.
    pub async fn ancestors(&self, id: Uuid) -> Result<Vec<BaseItemHierarchyEntry>, BaseItemError> {
        let closure = ancestor_id::Entity::find()
            .filter(ancestor_id::Column::ItemId.eq(id))
            .order_by_asc(ancestor_id::Column::Depth)
            .all(&self.database)
            .await?;
        hierarchy_entries(closure, false, &self.database).await
    }

    /// Loads all descendants in stable depth and identifier order.
    ///
    /// # Errors
    ///
    /// Returns a database error when either query fails.
    pub async fn descendants(
        &self,
        id: Uuid,
    ) -> Result<Vec<BaseItemHierarchyEntry>, BaseItemError> {
        let closure = ancestor_id::Entity::find()
            .filter(ancestor_id::Column::ParentItemId.eq(id))
            .order_by_asc(ancestor_id::Column::Depth)
            .order_by_asc(ancestor_id::Column::ItemId)
            .all(&self.database)
            .await?;
        hierarchy_entries(closure, true, &self.database).await
    }

    /// Loads `BoxSet` items containing a specific child via manual linked children.
    ///
    /// `PostgreSQL` performs the relationship lookup and stable Jellyfin sort
    /// in one indexed join. The source child is not loaded here; callers can
    /// first validate item visibility in the appropriate user context.
    ///
    /// # Errors
    ///
    /// Returns a database error when the relationship query fails.
    pub async fn collections_containing_item(
        &self,
        item_id: Uuid,
        start_index: u64,
        limit: Option<u64>,
    ) -> Result<BaseItemPage, BaseItemError> {
        let total_record_count = self
            .database
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r"
                SELECT COUNT(*)::bigint AS total_record_count
                FROM jellyfin.base_items AS collection
                WHERE collection.item_type = 'BoxSet'
                    AND EXISTS (
                        SELECT 1
                        FROM jellyfin.linked_children AS link
                        WHERE link.parent_id = collection.id
                            AND link.child_id = $1
                            AND link.child_type = 0
                    )
                ",
                [item_id.into()],
            ))
            .await?
            .map(|row| row.try_get::<i64>("", "total_record_count"))
            .transpose()?
            .unwrap_or(0);

        let mut values = vec![item_id.into(), (start_index as i64).into()];
        let mut sql = format!(
            r"
            SELECT {BASE_ITEM_COLUMNS}
            FROM jellyfin.base_items AS item
            WHERE item.item_type = 'BoxSet'
                AND EXISTS (
                    SELECT 1
                    FROM jellyfin.linked_children AS link
                    WHERE link.parent_id = item.id
                        AND link.child_id = $1
                        AND link.child_type = 0
                )
            ORDER BY lower(COALESCE(item.sort_name, item.name, '')) ASC,
                lower(COALESCE(item.name, '')) ASC,
                item.id ASC
            OFFSET $2
            "
        );
        if let Some(limit) = limit {
            values.push((limit as i64).into());
            sql.push_str(" LIMIT $3");
        }

        let items = base_item::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .all(&self.database)
        .await?;
        Ok(BaseItemPage {
            items,
            total_record_count: u64::try_from(total_record_count).unwrap_or(0),
            start_index,
        })
    }
}

const BASE_ITEM_COLUMNS: &str = "id, item_type, data, path, parent_id, top_parent_id, name, \
    clean_name, sort_name, media_type, overview, official_rating, index_number, parent_index_number, production_year, \
    premiere_date, runtime_ticks, is_folder, is_virtual_item, presentation_unique_key, primary_version_id, series_id, season_id, \
    series_presentation_unique_key, date_created, date_modified, row_version";

fn grouped_versions_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.* FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER'",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), version_groups AS (\
             (SELECT DISTINCT ON (presentation_unique_key) filtered.* \
              FROM filtered \
              WHERE presentation_unique_key IS NOT NULL \
              ORDER BY presentation_unique_key, (primary_version_id IS NULL) DESC, id) \
             UNION ALL \
             SELECT filtered.* FROM filtered WHERE presentation_unique_key IS NULL\
         )",
    );
    (sql, values)
}

fn resumable_filtered_cte(user_id: Uuid, query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = vec![user_id.into()];
    let mut sql = String::from(
        "WITH progress_by_item AS (\
             SELECT item_id, MAX(last_played_date) AS resume_last_played_date \
             FROM jellyfin.user_data \
             WHERE user_id = $1 AND playback_position_ticks > 0 \
             GROUP BY item_id\
         ), resume_versions AS (\
             SELECT DISTINCT ON (COALESCE(item.primary_version_id, item.id)) \
                    item.*, progress.resume_last_played_date \
             FROM progress_by_item AS progress \
             JOIN jellyfin.base_items AS item ON item.id = progress.item_id \
             WHERE item.item_type <> 'PLACEHOLDER' \
             ORDER BY COALESCE(item.primary_version_id, item.id), \
                      progress.resume_last_played_date DESC NULLS LAST, item.id\
         ), filtered AS (\
             SELECT item.* FROM resume_versions AS item \
             WHERE item.item_type <> 'PLACEHOLDER'",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push(')');
    (sql, values)
}

fn not_resumable_filtered_cte(user_id: Uuid, query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = vec![user_id.into()];
    let mut sql = String::from(
        "WITH resumable_groups AS (\
             SELECT DISTINCT COALESCE(item.primary_version_id, item.id) AS primary_id \
             FROM jellyfin.user_data AS progress \
             JOIN jellyfin.base_items AS item ON item.id = progress.item_id \
             WHERE progress.user_id = $1 AND progress.playback_position_ticks > 0\
         ), filtered AS (\
             SELECT item.* FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.primary_version_id IS NULL \
               AND NOT EXISTS (\
                   SELECT 1 FROM resumable_groups \
                   WHERE resumable_groups.primary_id = item.id\
               )",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push(')');
    (sql, values)
}

fn date_played_filtered_cte(user_id: Uuid, query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = vec![user_id.into()];
    let mut sql = String::from(
        "WITH version_dates AS (\
             SELECT COALESCE(item.primary_version_id, item.id) AS primary_id, \
                    MAX(progress.last_played_date) AS date_played \
             FROM jellyfin.user_data AS progress \
             JOIN jellyfin.base_items AS item ON item.id = progress.item_id \
             WHERE progress.user_id = $1 AND progress.last_played_date IS NOT NULL \
             GROUP BY COALESCE(item.primary_version_id, item.id)\
         ), filtered AS (\
             SELECT item.* FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.primary_version_id IS NULL",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), dated AS (\
             SELECT item.*, version_dates.date_played \
             FROM filtered AS item \
             LEFT JOIN version_dates ON version_dates.primary_id = item.id\
         )",
    );
    (sql, values)
}

fn extended_sort_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.*, \
                    COALESCE((
                        SELECT MAX(progress.play_count) \
                        FROM jellyfin.user_data AS progress \
                        WHERE progress.item_id = item.id \
                          AND progress.user_id = ",
    );
    let Some(user_id) = query.user_id else {
        unreachable!("extended-sort queries require a user id");
    };
    values.push(user_id.into());
    let _ = write!(sql, "${}", values.len());
    sql.push_str(
        "), 0)::bigint AS play_count, \
         COALESCE((item.data ->> 'CommunityRating')::double precision, 0.0) AS community_rating, \
         COALESCE((item.data ->> 'CriticRating')::double precision, 0.0) AS critic_rating \
         FROM jellyfin.base_items AS item \
         WHERE item.item_type <> 'PLACEHOLDER'",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push(')');
    (sql, values)
}

fn production_years_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.production_year \
             FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.production_year > 0",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), years AS (\
             SELECT DISTINCT production_year FROM filtered\
         )",
    );
    (sql, values)
}

fn official_ratings_cte(query: &BaseItemQuery) -> (String, Vec<SeaValue>) {
    let mut values = Vec::new();
    let mut sql = String::from(
        "WITH filtered AS (\
             SELECT item.official_rating \
             FROM jellyfin.base_items AS item \
             WHERE item.item_type <> 'PLACEHOLDER' \
               AND item.official_rating IS NOT NULL \
               AND item.official_rating <> ''",
    );
    append_raw_item_filters(&mut sql, &mut values, query);
    sql.push_str(
        "), ratings AS (\
             SELECT DISTINCT official_rating FROM filtered\
         )",
    );
    (sql, values)
}

#[allow(clippy::too_many_lines)]
fn append_raw_item_filters(sql: &mut String, values: &mut Vec<SeaValue>, query: &BaseItemQuery) {
    if !query.ids.is_empty() {
        append_uuid_list_filter(sql, values, "item.id", &query.ids);
    }
    if !query.exclude_ids.is_empty() {
        append_uuid_list_filter_with_operator(sql, values, "item.id", &query.exclude_ids, "NOT IN");
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
    if let Some(search_term) = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
    {
        let clean_search_term = search_term.clean_value();
        push_bind(
            sql,
            values,
            postgres_contains_pattern(&clean_search_term),
            " AND item.clean_name ILIKE ",
        );
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
    if let Some(is_virtual_item) = query.is_virtual_item {
        push_bind(sql, values, is_virtual_item, " AND item.is_virtual_item = ");
    }
    if let Some(is_played) = query.is_played {
        let Some(user_id) = query.user_id else {
            return;
        };
        if is_played {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE data.played = true AND data.user_id = ",
            );
        } else {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id NOT IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE data.played = true AND data.user_id = ",
            );
        }
        sql.push(')');
    }
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
    if let Some(is_folder) = query.is_folder {
        push_bind(sql, values, is_folder, " AND item.is_folder = ");
    }
    if let Some(is_liked) = query.is_liked {
        let Some(user_id) = query.user_id else {
            return;
        };
        if is_liked {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE data.likes = true AND data.user_id = ",
            );
        } else {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id NOT IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE data.likes = true AND data.user_id = ",
            );
        }
        sql.push(')');
    }
    if let Some(is_favorite_or_liked) = query.is_favorite_or_liked {
        let Some(user_id) = query.user_id else {
            return;
        };
        if is_favorite_or_liked {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE (data.is_favorite = true OR data.likes = true)
                      AND data.user_id = ",
            );
        } else {
            push_bind(
                sql,
                values,
                user_id,
                " AND item.id NOT IN (
                    SELECT data.item_id FROM jellyfin.user_data AS data
                    WHERE (data.is_favorite = true OR data.likes = true)
                      AND data.user_id = ",
            );
        }
        sql.push(')');
    }
    if !query.genres.is_empty() {
        sql.push_str(" AND EXISTS (");
        sql.push_str(&item_value_exists_sql(
            item_value::ItemValueType::Genre,
            &query.genres,
            values,
        ));
        sql.push(')');
    }
    if !query.tags.is_empty() {
        sql.push_str(" AND EXISTS (");
        sql.push_str(&item_value_exists_sql(
            item_value::ItemValueType::Tags,
            &query.tags,
            values,
        ));
        sql.push(')');
    }
    if !query.years.is_empty() {
        append_i32_list_filter(sql, values, "item.production_year", &query.years);
    }
    if let Some(person) = query.person.as_deref().filter(|value| !value.trim().is_empty()) {
        push_bind(
            sql,
            values,
            person,
            " AND EXISTS (
                SELECT 1 FROM jellyfin.people_base_item_map AS person_map
                JOIN jellyfin.people AS person ON person.id = person_map.person_id
                WHERE person_map.item_id = item.id AND person.name = ",
        );
        sql.push(')');
    }
    if let Some(min_community_rating) = query.min_community_rating {
        push_bind(
            sql,
            values,
            min_community_rating,
            " AND COALESCE((item.data ->> 'CommunityRating')::double precision, 0.0) >= ",
        );
    }
    if let Some(min_premiere_date) = query.min_premiere_date {
        push_bind(
            sql,
            values,
            min_premiere_date,
            " AND item.premiere_date >= ",
        );
    }
}

fn tag_class_condition(expected: bool, clean_tag: &'static str) -> sea_orm::sea_query::SimpleExpr {
    let expression = tag_class_expression("\"base_items\".\"id\"", clean_tag);
    if expected {
        Expr::cust(expression)
    } else {
        Expr::cust(format!("NOT {expression}"))
    }
}

fn media_class_condition(
    expected: bool,
    json_key: &'static str,
    item_types: &'static [&'static str],
) -> sea_orm::sea_query::SimpleExpr {
    let expression = media_class_expression("", json_key, item_types);
    if expected {
        Expr::cust(expression)
    } else {
        Expr::cust(format!("NOT {expression}"))
    }
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
    let expression = media_class_expression("item.", json_key, item_types);
    if expected {
        sql.push_str(" AND ");
        sql.push_str(&expression);
    } else {
        sql.push_str(" AND NOT ");
        sql.push_str(&expression);
    }
}

fn media_class_expression(
    prefix: &str,
    json_key: &'static str,
    item_types: &'static [&'static str],
) -> String {
    let mut expression = String::from("(");
    let _ = write!(expression, "{prefix}item_type IN (");
    for (index, item_type) in item_types.iter().enumerate() {
        if index > 0 {
            expression.push_str(", ");
        }
        expression.push('\'');
        expression.push_str(item_type);
        expression.push('\'');
    }
    let _ = write!(
        expression,
        ") OR COALESCE(lower({prefix}data ->> '{json_key}') = 'true', false))"
    );
    expression
}

fn append_tag_class_filter(sql: &mut String, expected: Option<bool>, clean_tag: &'static str) {
    let Some(expected) = expected else {
        return;
    };
    let expression = tag_class_expression("item.id", clean_tag);
    if expected {
        sql.push_str(" AND ");
        sql.push_str(&expression);
    } else {
        sql.push_str(" AND NOT ");
        sql.push_str(&expression);
    }
}

fn tag_class_expression(item_id_expression: &'static str, clean_tag: &'static str) -> String {
    format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS tag_map \
            JOIN jellyfin.item_values AS tag_value \
              ON tag_value.item_value_id = tag_map.item_value_id \
            WHERE tag_map.item_id = {item_id_expression} \
              AND tag_value.type = 4 \
              AND tag_value.clean_value = '{clean_tag}'\
        )"
    )
}

fn item_value_exists_expression(
    table: &'static str,
    item_column: &'static str,
    value_type: item_value::ItemValueType,
    values: &[String],
) -> sea_orm::sea_query::SimpleExpr {
    let type_code = item_value_type_code(value_type);
    let mut placeholders = String::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            placeholders.push_str(", ");
        }
        placeholders.push('\'');
        placeholders.push_str(&value.replace('\'', "''"));
        placeholders.push('\'');
    }
    Expr::cust(format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.item_value_map AS value_map \
            JOIN jellyfin.item_values AS item_value \
              ON item_value.item_value_id = value_map.item_value_id \
            WHERE value_map.item_id = {table}.{item_column} \
              AND item_value.type = {type_code} \
              AND item_value.value IN ({placeholders})\
        )"
    ))
}

fn item_value_exists_sql(
    value_type: item_value::ItemValueType,
    values: &[String],
    bind_values: &mut Vec<SeaValue>,
) -> String {
    let type_code = item_value_type_code(value_type);
    let mut sql = String::from(
        "SELECT 1 FROM jellyfin.item_value_map AS value_map \
         JOIN jellyfin.item_values AS item_value \
           ON item_value.item_value_id = value_map.item_value_id \
         WHERE value_map.item_id = item.id \
           AND item_value.type = ",
    );
    let _ = write!(sql, "{type_code} AND item_value.value IN (");
    append_bind_list(&mut sql, bind_values, values.iter().cloned());
    sql.push(')');
    sql
}

fn person_exists_expression(name: &str) -> sea_orm::sea_query::SimpleExpr {
    Expr::cust(format!(
        "EXISTS (\
            SELECT 1 FROM jellyfin.people_base_item_map AS person_map \
            JOIN jellyfin.people AS person ON person.id = person_map.person_id \
            WHERE person_map.item_id = base_items.id AND person.name = '{}'\
        )",
        name.replace('\'', "''")
    ))
}

fn community_rating_expression(minimum: f64) -> sea_orm::sea_query::SimpleExpr {
    Expr::cust(format!(
        "COALESCE((base_items.data ->> 'CommunityRating')::double precision, 0.0) >= {minimum}"
    ))
}

fn item_value_type_code(value_type: item_value::ItemValueType) -> i16 {
    match value_type {
        item_value::ItemValueType::Artist => 0,
        item_value::ItemValueType::AlbumArtist => 1,
        item_value::ItemValueType::Genre => 2,
        item_value::ItemValueType::Studios => 3,
        item_value::ItemValueType::Tags => 4,
        item_value::ItemValueType::InheritedTags => 6,
    }
}

fn append_i32_list_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    column: &str,
    items: &[i32],
) {
    if items.is_empty() {
        return;
    }
    let _ = write!(sql, " AND {column} IN (");
    append_bind_list(sql, values, items.iter().copied());
    sql.push(')');
}

fn append_uuid_list_filter(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    column: &str,
    items: &[Uuid],
) {
    append_uuid_list_filter_with_operator(sql, values, column, items, "IN");
}

fn append_uuid_list_filter_with_operator(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    column: &str,
    items: &[Uuid],
    operator: &str,
) {
    let _ = write!(sql, " AND {column} {operator} (");
    append_bind_list(sql, values, items.iter().copied());
    sql.push(')');
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
    let _ = write!(sql, " AND {column} {operator} (");
    append_bind_list(sql, values, items.iter().cloned());
    sql.push(')');
}

fn append_bind_list<T>(
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
        let _ = write!(sql, "${}", values.len());
    }
}

fn total_count_enabled(query: &BaseItemQuery) -> bool {
    query.enable_total_record_count.unwrap_or(true)
}

fn page_total_record_count(total_record_count: Option<u64>, item_count: usize) -> u64 {
    total_record_count.unwrap_or_else(|| u64::try_from(item_count).unwrap_or(u64::MAX))
}

fn push_bind<T: Into<SeaValue>>(
    sql: &mut String,
    values: &mut Vec<SeaValue>,
    value: T,
    prefix: &str,
) {
    values.push(value.into());
    let _ = write!(sql, "{prefix}${}", values.len());
}

fn postgres_contains_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    escaped
}

async fn acquire_hierarchy_lock(transaction: &DatabaseTransaction) -> Result<(), BaseItemError> {
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [HIERARCHY_ADVISORY_LOCK_KEY.into()],
        ))
        .await?;
    Ok(())
}

async fn validate_parent(
    transaction: &DatabaseTransaction,
    item_id: Uuid,
    parent_id: Option<Uuid>,
) -> Result<(), BaseItemError> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    if parent_id == item_id {
        return Err(BaseItemError::HierarchyCycle);
    }
    if base_item::Entity::find_by_id(parent_id)
        .one(transaction)
        .await?
        .is_none()
    {
        return Err(BaseItemError::ParentNotFound);
    }
    if ancestor_id::Entity::find_by_id((parent_id, item_id))
        .one(transaction)
        .await?
        .is_some()
    {
        return Err(BaseItemError::HierarchyCycle);
    }
    Ok(())
}

async fn hierarchy_entries(
    closure: Vec<ancestor_id::Model>,
    use_item_id: bool,
    database: &DatabaseConnection,
) -> Result<Vec<BaseItemHierarchyEntry>, BaseItemError> {
    let ids: Vec<Uuid> = closure
        .iter()
        .map(|row| {
            if use_item_id {
                row.item_id
            } else {
                row.parent_item_id
            }
        })
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let items = base_item::Entity::find()
        .filter(base_item::Column::Id.is_in(ids))
        .all(database)
        .await?;
    let mut by_id: HashMap<Uuid, base_item::Model> =
        items.into_iter().map(|item| (item.id, item)).collect();
    Ok(closure
        .into_iter()
        .filter_map(|row| {
            let id = if use_item_id {
                row.item_id
            } else {
                row.parent_item_id
            };
            by_id.remove(&id).map(|item| BaseItemHierarchyEntry {
                item,
                depth: row.depth,
            })
        })
        .collect())
}

fn validate_item_type(item_type: &str) -> Result<(), BaseItemError> {
    if item_type.trim().is_empty() {
        Err(BaseItemError::InvalidItemType)
    } else {
        Ok(())
    }
}

fn map_database_error(error: DbErr) -> BaseItemError {
    let message = error.to_string();
    if message.contains("base_items_hierarchy_acyclic")
        || message.contains("base_items_parent_not_self")
    {
        BaseItemError::HierarchyCycle
    } else if matches!(
        error.sql_err(),
        Some(SqlErr::ForeignKeyConstraintViolation(_))
    ) && message.contains("base_items_parent_id_fkey")
    {
        BaseItemError::ParentNotFound
    } else {
        BaseItemError::Database(error)
    }
}
