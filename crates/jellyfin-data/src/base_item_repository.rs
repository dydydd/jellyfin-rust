use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

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

use crate::entities::{ancestor_id, base_item, user_data};

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
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
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
            index_number: None,
            parent_index_number: None,
            production_year: None,
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
    DateCreatedDescending,
    DatePlayedAscending,
    DatePlayedDescending,
    Random,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseItemQuery {
    pub ids: Vec<Uuid>,
    pub exclude_ids: Vec<Uuid>,
    pub parent_id: Option<Uuid>,
    pub recursive: bool,
    pub search_term: Option<String>,
    pub include_item_types: Vec<String>,
    pub exclude_item_types: Vec<String>,
    pub media_types: Vec<String>,
    pub is_virtual_item: Option<bool>,
    pub group_versions_by_presentation_key: bool,
    pub user_id: Option<Uuid>,
    pub is_resumable: Option<bool>,
    pub is_played: Option<bool>,
    pub order: BaseItemOrder,
    pub start_index: u64,
    pub limit: Option<u64>,
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
            index_number: Set(item.index_number),
            parent_index_number: Set(item.parent_index_number),
            production_year: Set(item.production_year),
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
        let total_record_count = select.clone().count(&self.database).await?;
        let mut select = match query.order {
            BaseItemOrder::SortName => select.order_by_asc(base_item::Column::SortName),
            BaseItemOrder::DateCreatedDescending => {
                select.order_by_desc(base_item::Column::DateCreated)
            }
            BaseItemOrder::Random => select.order_by(Expr::cust("random()"), Order::Asc),
            BaseItemOrder::DatePlayedAscending | BaseItemOrder::DatePlayedDescending => {
                unreachable!("date-played queries are handled by query_by_date_played")
            }
        }
        .order_by_asc(base_item::Column::Id)
        .offset(query.start_index);
        if let Some(limit) = query.limit {
            select = select.limit(limit);
        }
        Ok(BaseItemPage {
            items: select.all(&self.database).await?,
            total_record_count,
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
        let count = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("{cte} SELECT COUNT(*) AS total_record_count FROM version_groups"),
                values.clone(),
            ))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound("grouped item count returned no row".to_owned()))?
            .try_get::<i64>("", "total_record_count")?;

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
            items,
            total_record_count: u64::try_from(count).unwrap_or_default(),
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
            BaseItemOrder::DateCreatedDescending => "date_created DESC, id",
            BaseItemOrder::SortName => "sort_name, id",
            BaseItemOrder::Random => "random(), id",
        };
        self.query_raw_page(cte, values, "dated", order, "DatePlayed", query)
            .await
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
        let count = transaction
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("{cte} SELECT COUNT(*) AS total_record_count FROM {source}"),
                values.clone(),
            ))
            .await?
            .ok_or_else(|| DbErr::RecordNotFound(format!("{query_name} count returned no row")))?
            .try_get::<i64>("", "total_record_count")?;

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
            items,
            total_record_count: u64::try_from(count).unwrap_or_default(),
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
            index_number: Set(item.index_number),
            parent_index_number: Set(item.parent_index_number),
            production_year: Set(item.production_year),
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
        user_data::Entity::delete_many()
            .filter(user_data::Column::ItemId.is_in(affected_ids))
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
}

const BASE_ITEM_COLUMNS: &str = "id, item_type, data, path, parent_id, top_parent_id, name, \
    clean_name, sort_name, media_type, overview, index_number, parent_index_number, production_year, \
    runtime_ticks, is_folder, is_virtual_item, presentation_unique_key, primary_version_id, series_id, season_id, \
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
